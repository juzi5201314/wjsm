// wjsm-module: ES Module / CommonJS bundling support
// 将多个模块编译为单一 WASM 二进制

mod resolver;
mod graph;
mod bundler;
mod semantic;
pub mod cjs_transform;

pub use resolver::{ModuleResolver, ResolvedModule, ImportEntry, ExportEntry};
pub use graph::{ModuleGraph, ModuleId};
pub use bundler::ModuleBundler;
pub use semantic::{ModuleLinkResult, analyze_module_links};

use anyhow::Result;
use std::path::Path;

/// Bundle entry module and all its dependencies into a single WASM binary
pub fn bundle(entry: &str, root_path: &Path) -> Result<Vec<u8>> {
    let mut bundler = ModuleBundler::new(root_path)?;
    bundler.bundle(entry)
}
