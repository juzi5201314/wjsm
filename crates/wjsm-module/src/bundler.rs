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

    /// Bundle 入口模块及其所有依赖
    pub fn bundle(&self, entry: &str) -> Result<Vec<u8>> {
        // 1. 构建依赖图
        let graph = ModuleGraph::build(entry, &self.root_path)
            .with_context(|| "Failed to build module graph")?;

        // 2. 获取拓扑排序顺序
        let (order, cycles) = graph
            .topological_order()
            .with_context(|| "Failed to compute topological order")?;

        // 循环依赖已在 topological_order 中记录，这里不输出到 stderr
        // 避免影响 fixture 测试的 stderr 快照比较
        let _ = cycles;

        // 3. 模块语义链接：收集导出并校验 import 绑定
        let link_result =
            analyze_module_links(&graph).with_context(|| "Failed to analyze module links")?;

        // 4. 收集所有模块的 AST
        let mut modules = Vec::new();

        for &id in &order {
            let node = graph.get_module(id).unwrap();
            modules.push((id, node.ast.clone()));
        }

        // 5. 调用语义层的多模块 lowering
        let program = wjsm_semantic::lower_modules(modules, &link_result.import_map)
            .with_context(|| "Failed to lower modules")?;

        // 6. 编译为 WASM
        let wasm_bytes =
            wjsm_backend_wasm::compile(&program).with_context(|| "Failed to compile to WASM")?;

        Ok(wasm_bytes)
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
