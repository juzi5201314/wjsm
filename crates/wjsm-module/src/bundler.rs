// 模块 Bundler：将多个模块编译为单一 WASM 二进制

use anyhow::{Context, Result};
use std::path::Path;

use super::graph::ModuleGraph;
use super::semantic::analyze_module_links;

/// 模块 Bundler
pub struct ModuleBundler {
    root_path: std::path::PathBuf,
}

impl ModuleBundler {
    pub fn new(root_path: &Path) -> Result<Self> {
        Ok(Self {
            root_path: root_path.to_path_buf(),
        })
    }

    /// 将入口模块及其依赖 lower 为 IR（不编译 WASM）
    pub fn lower_bundle(&self, entry: &str) -> Result<wjsm_ir::Program> {
        let graph = ModuleGraph::build(entry, &self.root_path)
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
            modules.push((id, node.ast.clone()));
        }

        wjsm_semantic::lower_modules(
            modules,
            &link_result.import_map,
            &link_result.dynamic_import_targets,
            &link_result.export_names,
            &link_result.dynamic_import_specifiers,
        )
        .with_context(|| "Failed to lower modules")
    }

    /// 解析入口模块 AST（含依赖图构建，用于 dump-ast 等）
    pub fn parse_entry_ast(&self, entry: &str) -> Result<swc_core::ecma::ast::Module> {
        let graph = ModuleGraph::build(entry, &self.root_path)
            .with_context(|| "Failed to build module graph")?;
        let entry_id = graph.entry_id();
        let node = graph
            .get_module(entry_id)
            .context("entry module missing from graph")?;
        Ok(node.ast.clone())
    }

    /// Bundle 入口模块及其所有依赖
    pub fn bundle(&self, entry: &str) -> Result<Vec<u8>> {
        let program = self
            .lower_bundle(entry)
            .with_context(|| "Failed to lower modules")?;

        wjsm_backend_wasm::compile(&program).with_context(|| "Failed to compile to WASM")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn bundler_new_creates_instance() {
        let root = PathBuf::from("/tmp");
        let bundler = ModuleBundler::new(&root);
        assert!(bundler.is_ok());
    }

    #[test]
    fn bundle_simple_modules_produces_wasm() {
        let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("fixtures/modules/simple");

        assert!(
            fixtures_dir.exists(),
            "Test fixtures not found at {:?}. Run from workspace root.",
            fixtures_dir
        );

        let bundler = ModuleBundler::new(&fixtures_dir).expect("bundler should be created");
        let result = bundler.bundle("./main.js");
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
}
