// wjsm-module: ES Module / CommonJS bundling support
// 将多个模块编译为单一 WASM 二进制

mod resolver;
mod graph;
mod bundler;
mod semantic;
pub mod cjs_transform;
use swc_core::ecma::ast;

pub use resolver::{ModuleResolver, ResolvedModule, ImportEntry, ExportEntry};
pub use graph::{ModuleGraph, ModuleId};
pub use bundler::ModuleBundler;
pub use semantic::{ModuleLinkResult, analyze_module_links};

use anyhow::Result;
use std::path::Path;

/// Bundle entry module and all its dependencies into a single WASM binary
pub fn bundle(entry: &str, root_path: &Path) -> Result<Vec<u8>> {
    let bundler = ModuleBundler::new(root_path)?;
    bundler.bundle(entry)
}

// ── 模块类型检测 ───────────────────────────────────────────────────

/// 检测 AST 是否包含 ES Module 语法（import/export 声明）
pub fn is_es_module(module: &ast::Module) -> bool {
    module.body.iter().any(|item| {
        matches!(
            item,
            ast::ModuleItem::ModuleDecl(
                ast::ModuleDecl::Import(_)
                    | ast::ModuleDecl::ExportDecl(_)
                    | ast::ModuleDecl::ExportNamed(_)
                    | ast::ModuleDecl::ExportDefaultDecl(_)
                    | ast::ModuleDecl::ExportDefaultExpr(_)
                    | ast::ModuleDecl::ExportAll(_)
            )
        )
    })
}

/// 检测 AST 是否包含 CommonJS 语法（require/exports/module.exports）
/// 代理到 cjs_transform::is_commonjs_module
pub fn is_commonjs_module(module: &ast::Module) -> bool {
    cjs_transform::is_commonjs_module(module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn public_bundle_function_works() {
        let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("fixtures/modules/simple");

        if !fixtures_dir.exists() {
            return;
        }

        let result = bundle("./main.js", &fixtures_dir);
        assert!(result.is_ok(), "public bundle should succeed: {:?}", result.err());
        let wasm_bytes = result.unwrap();
        assert!(!wasm_bytes.is_empty(), "should produce non-empty WASM output");
        assert!(wasm_bytes.starts_with(b"\x00asm"), "output should be valid WASM binary");
    }
}
