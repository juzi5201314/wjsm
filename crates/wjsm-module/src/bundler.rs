// 模块 Bundler：将多个模块编译为单一 WASM 二进制

use anyhow::{Context, Result, anyhow};
use std::path::Path;
use wjsm_semantic::{ModuleKind, ModuleLoweringInput, ModuleMetadata};

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

    /// Bundle 入口模块及其所有依赖
    pub fn bundle(&self, entry: &Path) -> Result<Vec<u8>> {
        let program = self
            .lower_bundle(entry)
            .with_context(|| "Failed to lower modules")?;

        wjsm_backend_wasm::compile_with_options(
            &program,
            wjsm_backend_wasm::CompileOptions {
                debug: self.emit_debug_checks,
            },
        )
        .with_context(|| "Failed to compile to WASM")
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

fn module_metadata_for_node(node: &super::graph::GraphNode) -> Result<ModuleMetadata> {
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
    fn bundle_simple_modules_produces_wasm() {
        let root = create_temp_project("simple_bundle");
        write_type_module_package(&root);
        write_file(
            &root,
            "main.js",
            "import { value } from './lib.js';\nconsole.log(value);\n",
        );
        write_file(&root, "lib.js", "export const value = 42;\n");

        let bundler = ModuleBundler::new(&root).expect("bundler should be created");
        let result = bundler.bundle(Path::new("main.js"));
        assert!(result.is_ok(), "bundle should succeed: {:?}", result.err());
        let wasm_bytes = result.unwrap();
        assert!(
            !wasm_bytes.is_empty(),
            "should produce non-empty WASM output"
        );
        assert!(
            wasm_bytes.starts_with(b"\x00asm"),
            "output should be valid WASM binary"
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
}
