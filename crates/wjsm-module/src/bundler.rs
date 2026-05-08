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
    pub fn bundle(&mut self, entry: &str) -> Result<Vec<u8>> {
        // 1. 构建依赖图
        let graph = ModuleGraph::build(entry, &self.root_path)
            .with_context(|| "Failed to build module graph")?;
        
        // 2. 获取拓扑排序顺序
        let order = graph.topological_order()
            .with_context(|| "Failed to compute topological order")?;
        
        // 3. 模块语义链接：收集导出并校验 import 绑定
        let link_result = analyze_module_links(&graph)
            .with_context(|| "Failed to analyze module links")?;

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
        let wasm_bytes = wjsm_backend_wasm::compile(&program)
            .with_context(|| "Failed to compile to WASM")?;
        
        Ok(wasm_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    
    #[test]
    fn test_bundle_simple() {
        // TODO: Add test after implementing lower_modules
    }
}
