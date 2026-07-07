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
}


/// 模块 Bundler
pub struct ModuleBundler {
    root_path: std::path::PathBuf,
    options: ResolutionOptions,
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
        })
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
            });
        }

        wjsm_semantic::lower_modules(
            modules,
            &link_result.import_map,
            &link_result.dynamic_import_targets,
            &link_result.export_names,
            &link_result.dynamic_import_specifiers,
            &link_result.re_export_map,
        )
        .with_context(|| "Failed to lower modules")
    }

    /// 将运行时加载的入口模块 lower 为可实例化 IR，并为入口 ESM 创建命名空间对象。
    pub fn lower_runtime_entry_bundle(&self, entry: &Path) -> Result<RuntimeEntryBundle> {
        let graph = ModuleGraph::build_with_options(entry, &self.root_path, self.options.clone())
            .with_context(|| "Failed to build module graph")?;
        let entry_module_id = graph.entry_id();
        let (order, cycles) = graph
            .topological_order()
            .with_context(|| "Failed to compute topological order")?;
        let _ = cycles;
        let mut link_result =
            analyze_module_links(&graph).with_context(|| "Failed to analyze module links")?;
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
            });
        }

        let program = wjsm_semantic::lower_modules(
            modules,
            &link_result.import_map,
            &link_result.dynamic_import_targets,
            &link_result.export_names,
            &link_result.dynamic_import_specifiers,
            &link_result.re_export_map,
        )
        .with_context(|| "Failed to lower modules")?;

        Ok(RuntimeEntryBundle {
            program,
            entry_module_id,
        })
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

        wjsm_backend_wasm::compile(&program).with_context(|| "Failed to compile to WASM")
    }
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
}
