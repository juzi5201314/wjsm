// 模块 Bundler：将多个模块 lower 为单一 semantic IR program

use anyhow::{Context, Result, anyhow};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use wjsm_artifact_format::{
    ArtifactBuildInput, BuildOptions, ManifestModule, ModuleKind as ArtifactModuleKind,
    ModuleManifest,
};
use wjsm_semantic::{ModuleKind, ModuleLoweringInput, ModuleMetadata};

use super::builtin_modules;
use super::graph::ModuleGraph;
use super::resolution_options::ResolutionOptions;
use super::semantic::analyze_module_links;
use super::source_store::ModuleSourceStore;
use wjsm_ir::{ModuleId, Program};

pub struct RuntimeEntryBundle {
    pub program: Program,
    pub manifest: ModuleManifest,
    pub entry_module_id: ModuleId,
    pub module_id_span: u32,
}

/// 一次 lower 的完整产物：`Program`、与其 ModuleId 布局共源的 manifest、
/// 入口模块源码（inspect 路径的 artifact source_text 用）。
struct LoweredBundleParts {
    program: Program,
    manifest: ModuleManifest,
    entry_source: Option<std::sync::Arc<str>>,
}

fn entry_source_for_graph(graph: &ModuleGraph) -> Option<std::sync::Arc<str>> {
    graph
        .get_module(graph.entry_id())
        .map(|module| std::sync::Arc::from(module.source.as_str()))
}

/// 模块 Bundler
pub struct ModuleBundler {
    store: ModuleSourceStore,
    options: ResolutionOptions,
    /// inspect 路径：在语句入口发射 DebugCheck。
    emit_debug_checks: bool,
}

impl ModuleBundler {
    pub fn new(root_path: &Path) -> Result<Self> {
        Self::with_resolution_options(root_path, ResolutionOptions::default())
    }

    /// Creates a module bundler with explicit package resolution options.
    pub fn with_resolution_options(root_path: &Path, options: ResolutionOptions) -> Result<Self> {
        Self::with_store(ModuleSourceStore::disk(root_path), options)
    }

    /// 用显式 store 构造 bundler（打包期 Recording / packed Snapshot）。
    pub fn with_store(store: ModuleSourceStore, options: ResolutionOptions) -> Result<Self> {
        Ok(Self {
            store,
            options,
            emit_debug_checks: false,
        })
    }

    pub fn store(&self) -> &ModuleSourceStore {
        &self.store
    }

    fn root_path(&self) -> PathBuf {
        self.store.root()
    }

    /// 启用语句级 debug 插桩（`--inspect`）。
    pub fn with_emit_debug_checks(mut self, enable: bool) -> Self {
        self.emit_debug_checks = enable;
        self
    }

    /// 将入口模块及其依赖 lower 为 IR，builtin 依赖闭包走独立 lower + 磁盘缓存
    /// （issue #344）。
    ///
    /// 流程：全图构建（现状）→ 按 [`builtin_modules::is_builtin_virtual_path`] 分片：
    /// 用户模块集 + builtin canonical frontier（用户模块直接 import/重导出/动态 import
    /// 的 builtin canonical 名，排序）。无 builtin frontier 时退化为 [`ModuleBundler::lower_bundle`]。
    /// 有 frontier 时：`${cache_dir}/builtin_ir/<key>.bin`（目录经
    /// [`crate::cache_dir::resolve_cache_dir`] 解析，缺省回落用户缓存目录）命中则反序列化段，
    /// 未命中则 [`crate::builtin_cache::build_builtin_segment`] 构建并落盘（磁盘缓存被禁用时
    /// 构建段但不落盘）；用户图重建（id 从段模块数起、共享 builtin canonical 节点）后经
    /// `wjsm_semantic::lower_modules_with_builtin_seed` 合并两段，返回合并后的 `Program`
    /// （builtin 段 ModuleId 0..B，用户模块 B..，两段不重叠）。
    ///
    /// 环境变量 `WJSM_NO_BUILTIN_CACHE` 非空时整体跳过缓存路径，直接走
    /// [`ModuleBundler::lower_bundle`]（调试/对比用）。
    pub fn lower_bundle_cached(&self, entry: &Path) -> Result<wjsm_ir::Program> {
        Ok(self.lower_bundle_cached_parts(entry)?.program)
    }

    /// 同 [`ModuleBundler::lower_bundle_cached`]，另返回与 `Program` 的
    /// ModuleId 布局一致的 manifest 与入口源码。
    ///
    /// manifest 必须与产出 `Program` 的同一图共源：缓存路径的合并布局是
    /// builtin 闭包 id 0..B + 用户 id B..，与全新 plain 图（入口 id 0 起
    /// 发现序分配）不同——错位的 manifest 会让运行时 (image, ModuleId) →
    /// RuntimeModuleKey 解析到错误的模块键。
    fn lower_bundle_cached_parts(&self, entry: &Path) -> Result<LoweredBundleParts> {
        if std::env::var_os("WJSM_NO_BUILTIN_CACHE").is_some() {
            return self.lower_bundle_parts(entry);
        }

        // 1) 全图构建（现状），用于分片。
        let graph = ModuleGraph::build_with_store(entry, self.store.clone(), self.options.clone())
            .with_context(|| "Failed to build module graph")?;

        // 2) builtin canonical frontier：用户模块直接引用（import/重导出/动态 import）的 builtin。
        let mut frontier = BTreeSet::new();
        for id in graph.all_module_ids() {
            let node = graph.get_module(id).expect("graph node missing");
            if builtin_modules::is_builtin_virtual_path(&node.path) {
                continue;
            }
            let mut note_builtin_dep = |dep_id: ModuleId| {
                if let Some(canonical) = graph
                    .get_module(dep_id)
                    .and_then(|dep| builtin_modules::canonical_from_virtual_path(&dep.path))
                {
                    frontier.insert(canonical.to_string());
                }
            };
            for (dep_id, _) in &node.imports {
                note_builtin_dep(*dep_id);
            }
            for (_, dep_id) in &node.dynamic_imports {
                note_builtin_dep(*dep_id);
            }
        }

        // 3) 无 builtin 依赖 → 现状 lower（无缓存路径），manifest 与该图共源。
        if frontier.is_empty() {
            return self.lower_graph_parts(&graph);
        }

        // 3.5) 用户程序含 top-level await（TLA）→ 回退现状 lower。
        // TLA 会把用户入口编成 async 状态机；#344 起约定 TLA / WJSM_NO_BUILTIN_CACHE /
        // 无 frontier 走整包单 image。双 image 路径不覆盖 TLA。
        let user_tla = graph.all_module_ids().any(|id| {
            let node = graph.get_module(id).expect("graph node missing");
            !builtin_modules::is_builtin_virtual_path(&node.path)
                && wjsm_semantic::program_has_top_level_await(&node.ast)
        });
        if user_tla {
            return self.lower_graph_parts(&graph);
        }

        // 4) 缓存键 + 目录；磁盘缓存被禁用时构建段但不落盘。
        let key = crate::builtin_cache::builtin_cache_key(&frontier, self.emit_debug_checks)?;
        let cache_dir = crate::cache_dir::resolve_cache_dir().map(|root| root.join("builtin_ir"));
        let segment = if let Some(segment) = crate::builtin_cache::load_memory_segment(&key) {
            segment
        } else {
            match cache_dir.as_deref() {
                Some(dir) => match crate::builtin_cache::load_builtin_segment(dir, &key) {
                    Some(segment) => segment,
                    None => {
                        let segment = crate::builtin_cache::build_builtin_segment(
                            &frontier,
                            &self.root_path(),
                            &self.options,
                            self.emit_debug_checks,
                        )
                        .with_context(|| "Failed to build builtin segment")?;
                        let _ = crate::builtin_cache::store_builtin_segment(dir, &key, &segment);
                        crate::builtin_cache::remember_builtin_segment(&key, &segment);
                        segment
                    }
                },
                None => {
                    let segment = crate::builtin_cache::build_builtin_segment(
                        &frontier,
                        &self.root_path(),
                        &self.options,
                        self.emit_debug_checks,
                    )
                    .with_context(|| "Failed to build builtin segment")?;
                    crate::builtin_cache::remember_builtin_segment(&key, &segment);
                    segment
                }
            }
        };

        // 5) 重建 canonical 闭包图（确定性纯解析，id 布局与段一致），共享节点进用户图。
        let closure = ModuleGraph::build_builtin_closure_with_store(
            &frontier,
            self.store.clone(),
            self.options.clone(),
        )
        .with_context(|| "Failed to rebuild builtin closure graph")?;
        let user_graph = ModuleGraph::build_user_with_builtin_closure_store(
            entry,
            self.store.clone(),
            self.options.clone(),
            &closure,
        )
        .with_context(|| "Failed to build user graph with shared builtin nodes")?;

        // 6) 链接分析（builtin 与用户 id 分离、不重叠）。
        let link_result =
            analyze_module_links(&user_graph).with_context(|| "Failed to analyze module links")?;
        let (order, cycles) = user_graph
            .topological_order()
            .with_context(|| "Failed to compute topological order")?;
        let _ = cycles;

        // 7) 用户模块 lower：以 builtin 段为种子合并（builtin 节点只参与链接分析，不重复 lower）。
        let mut modules = Vec::new();
        for &id in &order {
            let node = user_graph.get_module(id).expect("graph node missing");
            if builtin_modules::is_builtin_virtual_path(&node.path) {
                continue;
            }
            modules.push(ModuleLoweringInput {
                id: node.id,
                ast: node.ast.clone(),
                metadata: module_metadata_for_node(node, &self.store)?,
                source: Some(std::sync::Arc::<str>::from(node.source.as_str())),
            });
        }

        let program = wjsm_semantic::lower_modules_with_builtin_seed(
            modules,
            link_result.as_linking(),
            segment.to_semantic_segment(),
            self.emit_debug_checks,
        )
        .with_context(|| "Failed to lower user modules with builtin seed")?;
        // manifest 与合并布局共源：用户图含共享的 builtin canonical 节点
        //（id 0..B）+ 用户模块（id B..），覆盖合并 Program 的全部 ModuleId。
        let manifest = manifest_for_graph(&user_graph, &self.store, &self.options)?;
        let entry_source = entry_source_for_graph(&user_graph);
        Ok(LoweredBundleParts {
            program,
            manifest,
            entry_source,
        })
    }

    /// 将入口模块及其依赖 lower 为 IR（不执行 codegen）
    pub fn lower_bundle(&self, entry: &Path) -> Result<wjsm_ir::Program> {
        Ok(self.lower_bundle_parts(entry)?.program)
    }

    fn lower_bundle_parts(&self, entry: &Path) -> Result<LoweredBundleParts> {
        let graph = ModuleGraph::build_with_store(entry, self.store.clone(), self.options.clone())
            .with_context(|| "Failed to build module graph")?;
        self.lower_graph_parts(&graph)
    }

    fn lower_graph_parts(&self, graph: &ModuleGraph) -> Result<LoweredBundleParts> {
        let program = lower_graph(graph, &self.store, self.emit_debug_checks)?;
        let manifest = manifest_for_graph(graph, &self.store, &self.options)?;
        let entry_source = entry_source_for_graph(graph);
        Ok(LoweredBundleParts {
            program,
            manifest,
            entry_source,
        })
    }

    /// 将入口模块及其依赖 lower 为 portable artifact 的完整 target-independent 输入。
    pub fn lower_artifact_input(&self, entry: &Path) -> Result<ArtifactBuildInput> {
        let parts = self.lower_bundle_cached_parts(entry)?;
        let mut input = ArtifactBuildInput::new(
            parts.program,
            parts.manifest,
            BuildOptions {
                include_source_map: self.emit_debug_checks,
                include_source_text: self.emit_debug_checks,
            },
        );
        if self.emit_debug_checks {
            input.source_text = parts.entry_source;
        }
        Ok(input)
    }

    /// 将运行时加载的入口模块 lower 为可实例化 IR，并为入口 ESM 创建命名空间对象。
    pub fn lower_runtime_entry_bundle(&self, entry: &Path) -> Result<RuntimeEntryBundle> {
        let graph =
            ModuleGraph::build_runtime_with_store(entry, self.store.clone(), self.options.clone())
                .with_context(|| "Failed to build runtime module graph")?;
        lower_runtime_graph(&graph, &self.store, &self.options, self.emit_debug_checks)
    }

    /// 将 Node 内置模块 lower 为运行时可实例化 ESM bundle。
    pub fn lower_runtime_builtin_bundle(&self, specifier: &str) -> Result<RuntimeEntryBundle> {
        let graph = ModuleGraph::build_builtin_with_store(
            specifier,
            self.store.clone(),
            self.options.clone(),
        )
        .with_context(|| "Failed to build built-in module graph")?;
        lower_runtime_graph(&graph, &self.store, &self.options, self.emit_debug_checks)
    }

    /// 解析入口模块 AST（含依赖图构建，用于 dump-ast 等）
    pub fn parse_entry_ast(&self, entry: &Path) -> Result<swc_core::ecma::ast::Module> {
        let graph = ModuleGraph::build_with_store(entry, self.store.clone(), self.options.clone())
            .with_context(|| "Failed to build module graph")?;
        let entry_id = graph.entry_id();
        let node = graph
            .get_module(entry_id)
            .context("entry module missing from graph")?;
        Ok(node.ast.clone())
    }

    /// Bundle 入口模块及其所有依赖，产出 IR `Program`（codegen 由调用方负责）
    pub fn bundle_program(&self, entry: &Path) -> Result<wjsm_ir::Program> {
        self.lower_bundle(entry)
            .with_context(|| "Failed to lower modules")
    }
}

fn lower_graph(
    graph: &ModuleGraph,
    store: &ModuleSourceStore,
    emit_debug_checks: bool,
) -> Result<Program> {
    let (order, cycles) = graph
        .topological_order()
        .with_context(|| "Failed to compute topological order")?;
    let _ = cycles;
    let link_result =
        analyze_module_links(graph).with_context(|| "Failed to analyze module links")?;
    let mut modules = Vec::with_capacity(order.len());
    for id in order {
        let node = graph.get_module(id).expect("ordered graph node is present");
        modules.push(ModuleLoweringInput {
            id: node.id,
            ast: node.ast.clone(),
            metadata: module_metadata_for_node(node, store)?,
            source: Some(std::sync::Arc::<str>::from(node.source.as_str())),
        });
    }
    wjsm_semantic::lower_modules_with_debug(modules, link_result.as_linking(), emit_debug_checks)
        .with_context(|| "Failed to lower modules")
}

fn manifest_for_graph(
    graph: &ModuleGraph,
    store: &ModuleSourceStore,
    options: &ResolutionOptions,
) -> Result<ModuleManifest> {
    let canonical_root = store.root();
    let mut modules = Vec::new();
    for id in graph.all_module_ids() {
        let node = graph.get_module(id).expect("graph node is present");
        let (logical_url, kind) =
            if let Some(canonical) = builtin_modules::canonical_from_virtual_path(&node.path) {
                (format!("node:{canonical}"), ArtifactModuleKind::Builtin)
            } else {
                let relative = node.path.strip_prefix(&canonical_root).map_err(|_| {
                    anyhow!(
                        "module {} is outside build root {}",
                        node.path.display(),
                        canonical_root.display()
                    )
                })?;
                let logical_url = logical_url_from_path(relative)?;
                let kind = if node.is_cjs {
                    ArtifactModuleKind::CommonJs
                } else {
                    ArtifactModuleKind::EsModule
                };
                (logical_url, kind)
            };
        modules.push(ManifestModule {
            id: node.id,
            logical_url,
            kind,
            static_dependencies: node.imports.iter().map(|(id, _)| *id).collect(),
            dynamic_dependencies: node.dynamic_imports.clone(),
        });
    }
    Ok(ModuleManifest {
        entry: graph.entry_id(),
        modules,
        resolution_conditions: options.conditions().to_vec(),
    })
}

pub fn logical_url_from_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(encode_logical_component(part)?),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow!(
                    "module path is not root-relative: {}",
                    path.display()
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(anyhow!("module logical URL is empty"));
    }
    Ok(parts.join("/"))
}

fn encode_logical_component(component: &OsStr) -> Result<String> {
    let bytes = logical_component_bytes(component)?;
    let mut encoded = String::with_capacity(bytes.len());
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    Ok(encoded)
}

#[cfg(unix)]
fn logical_component_bytes(component: &OsStr) -> Result<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt as _;

    Ok(component.as_bytes().to_vec())
}

#[cfg(not(unix))]
fn logical_component_bytes(component: &OsStr) -> Result<Vec<u8>> {
    component
        .to_str()
        .map(|text| text.as_bytes().to_vec())
        .ok_or_else(|| anyhow!("module path component is not valid Unicode: {component:?}"))
}

pub fn logical_url_path(root: &Path, logical_url: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in logical_url.split('/') {
        let bytes = decode_logical_component(component)?;
        let component = logical_component_os_string(bytes)?;
        let mut components = Path::new(&component).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err(anyhow!("invalid logical module URL {logical_url:?}"));
        }
        path.push(component);
    }
    Ok(path)
}

fn decode_logical_component(component: &str) -> Result<Vec<u8>> {
    if component.is_empty() {
        return Err(anyhow!("logical module URL contains an empty component"));
    }
    let source = component.as_bytes();
    let mut decoded = Vec::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        if source[index] != b'%' {
            if !source[index].is_ascii() {
                return Err(anyhow!(
                    "logical module URL contains unescaped non-ASCII bytes"
                ));
            }
            decoded.push(source[index]);
            index += 1;
            continue;
        }
        let encoded = source
            .get(index + 1..index + 3)
            .ok_or_else(|| anyhow!("logical module URL contains truncated percent encoding"))?;
        let high = decode_hex(encoded[0])?;
        let low = decode_hex(encoded[1])?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    Ok(decoded)
}

fn decode_hex(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(anyhow!(
            "logical module URL contains invalid percent encoding"
        )),
    }
}

#[cfg(unix)]
fn logical_component_os_string(bytes: Vec<u8>) -> Result<OsString> {
    use std::os::unix::ffi::OsStringExt as _;

    Ok(OsString::from_vec(bytes))
}

#[cfg(not(unix))]
fn logical_component_os_string(bytes: Vec<u8>) -> Result<OsString> {
    String::from_utf8(bytes)
        .map(OsString::from)
        .map_err(|_| anyhow!("logical module URL is not valid UTF-8 on this platform"))
}

fn lower_runtime_graph(
    graph: &ModuleGraph,
    store: &ModuleSourceStore,
    options: &ResolutionOptions,
    emit_debug_checks: bool,
) -> Result<RuntimeEntryBundle> {
    let entry_module_id = graph.entry_id();
    let (order, cycles) = graph
        .topological_order()
        .with_context(|| "Failed to compute topological order")?;
    let _ = cycles;
    let module_id_span = module_id_span(&order)?;
    let mut link_result =
        analyze_module_links(graph).with_context(|| "Failed to analyze module links")?;
    link_result
        .dynamic_import_targets
        .entry(entry_module_id)
        .or_default()
        .push(entry_module_id);

    let mut modules = Vec::new();
    for &id in &order {
        let node = graph.get_module(id).unwrap();
        modules.push(ModuleLoweringInput {
            id: node.id,
            ast: node.ast.clone(),
            metadata: module_metadata_for_node(node, store)?,
            source: Some(std::sync::Arc::<str>::from(node.source.as_str())),
        });
    }

    let program = wjsm_semantic::lower_modules_with_debug(
        modules,
        link_result.as_linking(),
        emit_debug_checks,
    )
    .with_context(|| "Failed to lower modules")?;

    Ok(RuntimeEntryBundle {
        manifest: manifest_for_graph(graph, store, options)?,
        program,
        entry_module_id,
        module_id_span,
    })
}

fn module_id_span(order: &[ModuleId]) -> Result<u32> {
    let max_id = order.iter().map(|module_id| module_id.0).max().unwrap_or(0);
    max_id
        .checked_add(1)
        .ok_or_else(|| anyhow!("runtime module id span overflows u32"))
}

pub(crate) fn module_metadata_for_node(
    node: &super::graph::GraphNode,
    store: &ModuleSourceStore,
) -> Result<ModuleMetadata> {
    let kind = if node.is_cjs {
        ModuleKind::CommonJs
    } else {
        ModuleKind::Esm
    };
    if builtin_modules::is_builtin_virtual_path(&node.path) {
        return builtin_module_metadata(node, kind);
    }
    match store.module_identity(&node.path) {
        Ok((filename, dirname, url)) => Ok(ModuleMetadata {
            filename,
            dirname,
            url,
            kind,
        }),
        Err(error) if kind == ModuleKind::CommonJs => Err(error),
        Err(_) => Ok(ModuleMetadata {
            filename: String::new(),
            dirname: String::new(),
            url: String::new(),
            kind,
        }),
    }
}

fn builtin_module_metadata(
    node: &super::graph::GraphNode,
    kind: ModuleKind,
) -> Result<ModuleMetadata> {
    let dirname_path = node
        .path
        .parent()
        .ok_or_else(|| anyhow!("module path has no parent: {}", node.path.display()))?;
    let (filename, dirname, url) = match (path_to_utf8(&node.path), path_to_utf8(dirname_path)) {
        (Ok(filename), Ok(dirname)) => {
            let url = url::Url::from_file_path(&node.path)
                .map_err(|_| {
                    anyhow!(
                        "module path cannot be converted to file URL: {}",
                        node.path.display()
                    )
                })?
                .to_string();
            (filename, dirname, url)
        }
        (Err(error), _) | (_, Err(error)) if kind == ModuleKind::CommonJs => return Err(error),
        _ => (String::new(), String::new(), String::new()),
    };
    Ok(ModuleMetadata {
        filename,
        dirname,
        url,
        kind,
    })
}

fn path_to_utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("module path is not valid UTF-8: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Deref;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_PROJECT: AtomicUsize = AtomicUsize::new(0);

    struct TestProject {
        path: PathBuf,
    }

    impl TestProject {
        fn new(case: &str) -> Self {
            let id = NEXT_TEST_PROJECT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join("wjsm-test-cache")
                .join("module")
                .join(format!("bundler-{case}-{}-{id}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp project dir should be creatable");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Deref for TestProject {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            self.path()
        }
    }

    impl Drop for TestProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn create_temp_project(case: &str) -> TestProject {
        TestProject::new(case)
    }

    fn write_file(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent dir should be created");
        }
        std::fs::write(path, content).expect("fixture file should be writable");
    }

    fn write_type_module_package(root: &Path) {
        write_file(root, "package.json", r#"{"type":"module"}"#);
    }

    #[test]
    fn bundler_new_creates_instance() {
        let root = PathBuf::from("/tmp");
        let bundler = ModuleBundler::new(&root);
        assert!(bundler.is_ok());
    }

    #[test]
    fn bundle_simple_modules_produces_program() {
        let root = create_temp_project("simple_bundle");
        write_type_module_package(&root);
        write_file(
            &root,
            "main.js",
            "import { value } from './lib.js';\nconsole.log(value);\n",
        );
        write_file(&root, "lib.js", "export const value = 42;\n");

        let bundler = ModuleBundler::new(&root).expect("bundler should be created");
        let result = bundler.bundle_program(Path::new("main.js"));
        assert!(result.is_ok(), "bundle should succeed: {:?}", result.err());
        let program = result.unwrap();
        assert!(
            !program.functions().is_empty(),
            "bundled program should contain lowered functions"
        );
    }

    #[test]
    fn lower_bundle_re_export_chain() {
        let root = create_temp_project("re_export_bundle");
        write_type_module_package(&root);
        write_file(
            &root,
            "main.js",
            "import { x } from './re.js';\nconsole.log(x);\n",
        );
        write_file(
            &root,
            "re.js",
            "export { value as x } from './source.js';\n",
        );
        write_file(&root, "source.js", "export const value = 42;\n");

        let bundler = ModuleBundler::new(&root).expect("bundler");
        let result = bundler.lower_bundle(Path::new("main.js"));
        assert!(
            result.is_ok(),
            "re_export lower should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn lower_runtime_entry_bundle_keeps_static_dynamic_import_module_ids_local() {
        let root = create_temp_project("runtime_static_dynamic_import_ids");
        write_type_module_package(&root);
        write_file(
            &root,
            "main.mjs",
            "export function load() { let loaded; loaded = import('./dep.mjs'); return loaded; }\n",
        );
        write_file(&root, "dep.mjs", "export const value = 1;\n");

        let bundler = ModuleBundler::new(&root).expect("bundler");
        let bundle = bundler
            .lower_runtime_entry_bundle(Path::new("main.mjs"))
            .expect("runtime bundle should lower");
        let module_ids = module_id_constants(&bundle.program);

        assert_eq!(bundle.entry_module_id, ModuleId(0));
        assert_eq!(bundle.module_id_span, 2);
        assert!(
            module_ids.contains(&ModuleId(1)),
            "static import() fast path should retain dependency ModuleId constant: {module_ids:?}"
        );
    }

    fn module_id_constants(program: &Program) -> Vec<ModuleId> {
        program
            .constants()
            .iter()
            .filter_map(|constant| match constant {
                wjsm_ir::Constant::ModuleId(module_id) => Some(*module_id),
                _ => None,
            })
            .collect()
    }

    /// 设置 WJSM_CACHE_DIR 并返回清理句柄。测试进程内独占 env（Mutex 串行化）。
    fn with_cache_dir(tag: &str) -> (PathBuf, parking_lot::MutexGuard<'static, ()>) {
        use parking_lot::Mutex;
        static CACHE_ENV_LOCK: Mutex<()> = Mutex::new(());
        let guard = CACHE_ENV_LOCK.lock();
        let cache_dir = std::env::temp_dir()
            .join("wjsm-test-cache")
            .join("module")
            .join(format!("builtin-cache-e2e-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache_dir);
        // SAFETY: 测试进程内独占该 env（Mutex 串行化），与运行无关。
        unsafe { std::env::set_var("WJSM_CACHE_DIR", &cache_dir) };
        (cache_dir, guard)
    }

    /// 断言合并 Program 的布局契约（与 wjsm-semantic hydrate 对齐）：
    /// builtin 段函数原序在前（functions[0..F_b)），$builtin_main 在段内
    /// entry_function_id 位置；用户函数在后，$module_main 是最后一个函数。
    fn assert_merged_layout(
        program: &Program,
        segment: &crate::builtin_cache::BuiltinSegmentCacheFile,
    ) {
        let builtin_count = segment.program.functions().len();
        assert!(
            program.functions().len() > builtin_count,
            "合并 Program 应含用户函数: merged={} builtin={}",
            program.functions().len(),
            builtin_count
        );
        let segment_names: Vec<&str> = segment
            .program
            .functions()
            .iter()
            .map(|function| function.name())
            .collect();
        let merged_names: Vec<&str> = program
            .functions()
            .iter()
            .map(|function| function.name())
            .collect();
        assert_eq!(
            &merged_names[..builtin_count],
            segment_names.as_slice(),
            "builtin 段函数应原序前置"
        );
        let entry_index =
            usize::try_from(segment.entry_function_id.0).expect("u32 索引在 usize 内");
        assert_eq!(merged_names[entry_index], "$builtin_main");
        assert_eq!(
            merged_names.last().copied(),
            Some(wjsm_ir::MODULE_ENTRY_IR_NAME),
            "用户 $module_main 应为合并 Program 最后一个函数"
        );
    }

    /// E2E（任务指定 fixture）：`lower_bundle_cached` 对 perf_hooks 的分段布局 +
    /// 缓存命中（issue #344）。
    ///
    #[test]
    fn lower_bundle_cached_perf_hooks_layout_and_cache_hit() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/happy/node_builtin_perf_hooks_api_semantics.js");
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/happy");
        assert!(fixture.is_file(), "fixture 缺失: {}", fixture.display());

        let (cache_dir, _guard) = with_cache_dir("perf_hooks");
        let bundler = ModuleBundler::new(&root).expect("bundler");
        let program = bundler
            .lower_bundle_cached(&fixture)
            .expect("cached lower 应成功");
        program
            .verify()
            .expect("perf_hooks 合并 Program 应通过 IR 校验");

        // 独立构建段（同 frontier/options），对照合并布局。
        let frontier = ["perf_hooks"].into_iter().map(str::to_string).collect();
        let segment = crate::builtin_cache::build_builtin_segment(
            &frontier,
            &root,
            &ResolutionOptions::default(),
            false,
        )
        .expect("perf_hooks 段构建应成功");
        assert_merged_layout(&program, &segment);

        // 缓存落盘：同 key 文件存在；二次调用命中并产出同结构 Program。
        let key = crate::builtin_cache::builtin_cache_key(&frontier, false).unwrap();
        let cache_file = cache_dir.join("builtin_ir").join(format!("{key}.bin"));
        assert!(
            cache_file.is_file(),
            "缓存文件应落盘: {}",
            cache_file.display()
        );
        let second = bundler
            .lower_bundle_cached(&fixture)
            .expect("二次 cached lower 应命中缓存并成功");
        assert_eq!(
            second.functions().len(),
            program.functions().len(),
            "缓存命中的第二次 lower 应产出同结构 Program"
        );

        // 缓存失效：emit_debug_checks 不同 → 不同 key，重建后新 key 文件出现。
        let debug_bundler =
            ModuleBundler::with_resolution_options(&root, ResolutionOptions::default())
                .expect("bundler")
                .with_emit_debug_checks(true);
        let _debug_program = debug_bundler
            .lower_bundle_cached(&fixture)
            .expect("debug cached lower 应成功");
        let debug_key = crate::builtin_cache::builtin_cache_key(&frontier, true).unwrap();
        assert_ne!(debug_key, key, "emit_debug_checks 应改变缓存键");
        assert!(
            cache_dir
                .join("builtin_ir")
                .join(format!("{debug_key}.bin"))
                .is_file(),
            "debug 段缓存文件应落盘"
        );

        let _ = std::fs::remove_dir_all(&cache_dir);
    }

    /// E2E（verify 覆盖）：`lower_bundle_cached` 对 async_hooks fixture 产出通过
    /// IR 校验的合并 Program（async_hooks 闭包在基线上 verify 干净），并命中缓存。
    #[test]
    fn lower_bundle_cached_async_hooks_closure_verifies() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/happy/async_hooks_phase_order.js");
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/happy");
        assert!(fixture.is_file(), "fixture 缺失: {}", fixture.display());

        let (cache_dir, _guard) = with_cache_dir("async_hooks");
        let bundler = ModuleBundler::new(&root).expect("bundler");
        let program = bundler
            .lower_bundle_cached(&fixture)
            .expect("cached lower 应成功");
        program.verify().expect("合并 Program 应通过 IR 校验");

        let frontier = ["async_hooks"].into_iter().map(str::to_string).collect();
        let segment = crate::builtin_cache::build_builtin_segment(
            &frontier,
            &root,
            &ResolutionOptions::default(),
            false,
        )
        .expect("async_hooks 段构建应成功");
        assert_merged_layout(&program, &segment);

        let key = crate::builtin_cache::builtin_cache_key(&frontier, false).unwrap();
        assert!(
            cache_dir
                .join("builtin_ir")
                .join(format!("{key}.bin"))
                .is_file(),
            "async_hooks 段缓存文件应落盘"
        );
        let _ = std::fs::remove_dir_all(&cache_dir);
    }

    /// E2E（多入口闭包）：单个用户模块同时 import 两个 builtin → frontier 含两个
    /// canonical，闭包图多入口 BFS 合并 lower 成单段（id 0..B 按 frontier 排序分配）。
    #[test]
    fn lower_bundle_cached_multi_frontier_closure() {
        let root = create_temp_project("cached_multi_frontier");
        write_type_module_package(&root);
        write_file(
            &root,
            "main.mjs",
            "import { inspect } from 'node:util';\nimport { createHook } from 'node:async_hooks';\nconsole.log(inspect, typeof createHook);\n",
        );

        let (cache_dir, _guard) = with_cache_dir("multi_frontier");
        let bundler = ModuleBundler::new(&root).expect("bundler");
        let program = bundler
            .lower_bundle_cached(Path::new("main.mjs"))
            .expect("多入口 cached lower 应成功");

        // frontier 排序：[async_hooks, util]，两模块都进入闭包段。
        let frontier: BTreeSet<String> = ["async_hooks", "util"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let segment = crate::builtin_cache::build_builtin_segment(
            &frontier,
            &root,
            &ResolutionOptions::default(),
            false,
        )
        .expect("多入口段构建应成功");
        assert_merged_layout(&program, &segment);

        let canonicals: Vec<&str> = segment
            .modules
            .iter()
            .map(|record| record.canonical.as_str())
            .collect();
        assert!(
            canonicals.contains(&"async_hooks"),
            "闭包含 async_hooks: {canonicals:?}"
        );
        assert!(canonicals.contains(&"util"), "闭包含 util: {canonicals:?}");

        let key = crate::builtin_cache::builtin_cache_key(&frontier, false).unwrap();
        assert!(
            cache_dir
                .join("builtin_ir")
                .join(format!("{key}.bin"))
                .is_file(),
            "多入口段缓存文件应落盘"
        );
        let _ = std::fs::remove_dir_all(&cache_dir);
    }
}
