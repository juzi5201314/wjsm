// 模块 Bundler：将多个模块编译为单一 WASM 二进制

use anyhow::{Context, Result, anyhow};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use wjsm_semantic::{ModuleKind, ModuleLoweringInput, ModuleMetadata};

use super::builtin_modules;
use super::graph::ModuleGraph;
use super::resolution_options::ResolutionOptions;
use super::semantic::analyze_module_links;
use wjsm_ir::{ModuleId, Program};

pub struct RuntimeEntryBundle {
    pub program: Program,
    pub entry_module_id: ModuleId,
    pub module_id_span: u32,
}

/// 模块 Bundler
pub struct ModuleBundler {
    root_path: std::path::PathBuf,
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
        Ok(Self {
            root_path: root_path.to_path_buf(),
            options,
            emit_debug_checks: false,
        })
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
    /// 有 frontier 时：`${WJSM_CACHE_DIR}/builtin_ir/<key>.bin` 命中则反序列化段，未命中则
    /// [`crate::builtin_cache::build_builtin_segment`] 构建并落盘（`WJSM_CACHE_DIR` 未设置时
    /// 构建段但不落盘）；用户图重建（id 从段模块数起、共享 builtin canonical 节点）后经
    /// `wjsm_semantic::lower_modules_with_builtin_seed` 合并两段，返回合并后的 `Program`
    /// （builtin 段 ModuleId 0..B，用户模块 B..，两段不重叠）。
    ///
    /// 环境变量 `WJSM_NO_BUILTIN_CACHE` 非空时整体跳过缓存路径，直接走
    /// [`ModuleBundler::lower_bundle`]（调试/对比用）。
    pub fn lower_bundle_cached(&self, entry: &Path) -> Result<wjsm_ir::Program> {
        if std::env::var_os("WJSM_NO_BUILTIN_CACHE").is_some() {
            return self.lower_bundle(entry);
        }

        // 1) 全图构建（现状），用于分片。
        let graph = ModuleGraph::build_with_options(entry, &self.root_path, self.options.clone())
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

        // 3) 无 builtin 依赖 → 现状 lower（无缓存路径）。
        if frontier.is_empty() {
            return self.lower_bundle(entry);
        }

        // 3.5) 用户程序含 top-level await（TLA）→ 回退现状 lower。
        // builtin 顶层代码 inline 进用户 async 状态机（main$async）后，其模块变量
        // （如 `$2.match`）跨 await 存活所需的 ContinuationSaveVar/load_var 对不会
        // 重新生成（save_var 由语义层 async 编译时基于当时的 IR 生成，inline 是
        // IR 级后处理），恢复路径会读到未恢复的 local——TLA 程序的 builtin 缓存
        // 路径在语义上不等价。非 TLA 程序（含全部 perf_hooks fixture）不受影响。
        let user_tla = graph.all_module_ids().any(|id| {
            let node = graph.get_module(id).expect("graph node missing");
            !builtin_modules::is_builtin_virtual_path(&node.path)
                && wjsm_semantic::program_has_top_level_await(&node.ast)
        });
        if user_tla {
            return self.lower_bundle(entry);
        }

        // 4) 缓存键 + 目录；WJSM_CACHE_DIR 未设置时构建段但不落盘。
        let key = crate::builtin_cache::builtin_cache_key(&frontier, self.emit_debug_checks)?;
        let cache_dir = std::env::var_os("WJSM_CACHE_DIR")
            .map(PathBuf::from)
            .map(|root| root.join("builtin_ir"));
        let segment = match cache_dir.as_deref() {
            Some(dir) => match crate::builtin_cache::load_builtin_segment(dir, &key) {
                Some(segment) => segment,
                None => {
                    let segment = crate::builtin_cache::build_builtin_segment(
                        &frontier,
                        &self.root_path,
                        &self.options,
                        self.emit_debug_checks,
                    )
                    .with_context(|| "Failed to build builtin segment")?;
                    // 落盘失败不阻塞编译（与 CLI 的 wasm 缓存同一约定：缓存是尽力而为）。
                    let _ = crate::builtin_cache::store_builtin_segment(dir, &key, &segment);
                    segment
                }
            },
            None => crate::builtin_cache::build_builtin_segment(
                &frontier,
                &self.root_path,
                &self.options,
                self.emit_debug_checks,
            )
            .with_context(|| "Failed to build builtin segment")?,
        };

        // 5) 重建 canonical 闭包图（确定性纯解析，id 布局与段一致），共享节点进用户图。
        let closure = ModuleGraph::build_builtin_closure(
            &frontier,
            &self.root_path,
            self.options.clone(),
        )
        .with_context(|| "Failed to rebuild builtin closure graph")?;
        let user_graph = ModuleGraph::build_user_with_builtin_closure(
            entry,
            &self.root_path,
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
                metadata: module_metadata_for_node(node)?,
                source: Some(std::sync::Arc::<str>::from(node.source.as_str())),
            });
        }

        wjsm_semantic::lower_modules_with_builtin_seed(
            modules,
            &link_result.import_map,
            &link_result.dynamic_import_targets,
            &link_result.export_names,
            &link_result.dynamic_import_specifiers,
            &link_result.re_export_map,
            segment.to_semantic_segment(),
            self.emit_debug_checks,
        )
        .with_context(|| "Failed to lower user modules with builtin seed")
    }

    /// 将入口模块及其依赖 lower 为 IR（不编译 WASM）
    pub fn lower_bundle(&self, entry: &Path) -> Result<wjsm_ir::Program> {
        let graph = ModuleGraph::build_with_options(entry, &self.root_path, self.options.clone())
            .with_context(|| "Failed to build module graph")?;

        let (order, cycles) = graph
            .topological_order()
            .with_context(|| "Failed to compute topological order")?;
        let _ = cycles;

        let link_result =
            analyze_module_links(&graph).with_context(|| "Failed to analyze module links")?;

        let mut modules = Vec::new();
        for &id in &order {
            let node = graph.get_module(id).unwrap();
            modules.push(ModuleLoweringInput {
                id: node.id,
                ast: node.ast.clone(),
                metadata: module_metadata_for_node(node)?,
                source: Some(std::sync::Arc::<str>::from(node.source.as_str())),
            });
        }

        wjsm_semantic::lower_modules_with_debug(
            modules,
            &link_result.import_map,
            &link_result.dynamic_import_targets,
            &link_result.export_names,
            &link_result.dynamic_import_specifiers,
            &link_result.re_export_map,
            self.emit_debug_checks,
        )
        .with_context(|| "Failed to lower modules")
    }

    /// 将运行时加载的入口模块 lower 为可实例化 IR，并为入口 ESM 创建命名空间对象。
    pub fn lower_runtime_entry_bundle(&self, entry: &Path) -> Result<RuntimeEntryBundle> {
        let graph = ModuleGraph::build_with_options(entry, &self.root_path, self.options.clone())
            .with_context(|| "Failed to build module graph")?;
        lower_runtime_graph(&graph, self.emit_debug_checks)
    }

    /// 将 Node 内置模块 lower 为运行时可实例化 ESM bundle。
    pub fn lower_runtime_builtin_bundle(&self, specifier: &str) -> Result<RuntimeEntryBundle> {
        let graph = ModuleGraph::build_builtin_with_options(
            specifier,
            &self.root_path,
            self.options.clone(),
        )
        .with_context(|| "Failed to build built-in module graph")?;
        lower_runtime_graph(&graph, self.emit_debug_checks)
    }

    /// 解析入口模块 AST（含依赖图构建，用于 dump-ast 等）
    pub fn parse_entry_ast(&self, entry: &Path) -> Result<swc_core::ecma::ast::Module> {
        let graph = ModuleGraph::build_with_options(entry, &self.root_path, self.options.clone())
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

fn lower_runtime_graph(graph: &ModuleGraph, emit_debug_checks: bool) -> Result<RuntimeEntryBundle> {
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
            metadata: module_metadata_for_node(node)?,
            source: Some(std::sync::Arc::<str>::from(node.source.as_str())),
        });
    }

    let program = wjsm_semantic::lower_modules_with_debug(
        modules,
        &link_result.import_map,
        &link_result.dynamic_import_targets,
        &link_result.export_names,
        &link_result.dynamic_import_specifiers,
        &link_result.re_export_map,
        emit_debug_checks,
    )
    .with_context(|| "Failed to lower modules")?;

    Ok(RuntimeEntryBundle {
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

pub(crate) fn module_metadata_for_node(node: &super::graph::GraphNode) -> Result<ModuleMetadata> {
    let kind = if node.is_cjs {
        ModuleKind::CommonJs
    } else {
        ModuleKind::Esm
    };
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
            let path = std::env::temp_dir().join(format!(
                "wjsm_module_bundler_{case}_{}_{id}",
                std::process::id()
            ));
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
    fn lower_runtime_entry_bundle_keeps_static_dynamic_import_module_ids_offsettable() {
        let root = create_temp_project("runtime_static_dynamic_import_offset");
        write_type_module_package(&root);
        write_file(
            &root,
            "main.mjs",
            "export function load() { let loaded; loaded = import('./dep.mjs'); return loaded; }\n",
        );
        write_file(&root, "dep.mjs", "export const value = 1;\n");

        let bundler = ModuleBundler::new(&root).expect("bundler");
        let mut bundle = bundler
            .lower_runtime_entry_bundle(Path::new("main.mjs"))
            .expect("runtime bundle should lower");
        let module_ids = module_id_constants(&bundle.program);

        assert_eq!(bundle.entry_module_id, ModuleId(0));
        assert_eq!(bundle.module_id_span, 2);
        assert!(
            module_ids.contains(&ModuleId(1)),
            "static import() fast path should retain dependency ModuleId constant: {module_ids:?}"
        );

        bundle
            .program
            .offset_module_ids(100)
            .expect("runtime bundle ids should offset");
        let offset_module_ids = module_id_constants(&bundle.program);

        assert!(!offset_module_ids.contains(&ModuleId(0)));
        assert!(!offset_module_ids.contains(&ModuleId(1)));
        assert!(offset_module_ids.contains(&ModuleId(100)));
        assert!(offset_module_ids.contains(&ModuleId(101)));
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
        let cache_dir = std::env::temp_dir().join(format!(
            "wjsm-builtin-cache-e2e-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&cache_dir);
        // SAFETY: 测试进程内独占该 env（Mutex 串行化），与运行无关。
        unsafe { std::env::set_var("WJSM_CACHE_DIR", &cache_dir) };
        (cache_dir, guard)
    }

    /// 断言合并 Program 的布局契约（与 wjsm-semantic hydrate 对齐）：
    /// builtin 段函数原序在前（functions[0..F_b)），$builtin_main 在段内
    /// entry_function_id 位置；用户函数在后，$module_main 是最后一个函数。
    fn assert_merged_layout(program: &Program, segment: &crate::builtin_cache::BuiltinSegmentCacheFile) {
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
            &merged_names[..builtin_count], segment_names.as_slice(),
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
    /// fixture 用 `require("node:perf_hooks")` 引入 builtin，frontier = {perf_hooks}。
    /// 不做整体 verify()：perf_hooks 闭包在**基线提交（#344 之前）就存在**的
    /// `clearTimelineEntries` 死块（block has instructions but terminator is unreachable）
    /// 校验失败（wjsm-semantic 引擎问题，运行时 fixture 不受影响，见
    /// `lower_bundle_cached_async_hooks_closure_verifies` 的 verify 覆盖）。
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
        assert!(cache_file.is_file(), "缓存文件应落盘: {}", cache_file.display());
        let second = bundler
            .lower_bundle_cached(&fixture)
            .expect("二次 cached lower 应命中缓存并成功");
        assert_eq!(
            second.functions().len(),
            program.functions().len(),
            "缓存命中的第二次 lower 应产出同结构 Program"
        );

        // 缓存失效：emit_debug_checks 不同 → 不同 key，重建后新 key 文件出现。
        let debug_bundler = ModuleBundler::with_resolution_options(&root, ResolutionOptions::default())
            .expect("bundler")
            .with_emit_debug_checks(true);
        let _debug_program = debug_bundler
            .lower_bundle_cached(&fixture)
            .expect("debug cached lower 应成功");
        let debug_key = crate::builtin_cache::builtin_cache_key(&frontier, true).unwrap();
        assert_ne!(debug_key, key, "emit_debug_checks 应改变缓存键");
        assert!(
            cache_dir.join("builtin_ir").join(format!("{debug_key}.bin")).is_file(),
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
            cache_dir.join("builtin_ir").join(format!("{key}.bin")).is_file(),
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
        assert!(canonicals.contains(&"async_hooks"), "闭包含 async_hooks: {canonicals:?}");
        assert!(canonicals.contains(&"util"), "闭包含 util: {canonicals:?}");

        let key = crate::builtin_cache::builtin_cache_key(&frontier, false).unwrap();
        assert!(
            cache_dir.join("builtin_ir").join(format!("{key}.bin")).is_file(),
            "多入口段缓存文件应落盘"
        );
        let _ = std::fs::remove_dir_all(&cache_dir);
    }
}
