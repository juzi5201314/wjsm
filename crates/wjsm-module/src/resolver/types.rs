use std::path::PathBuf;

use swc_core::common::Span;
use swc_core::ecma::ast;

pub use wjsm_ir::ModuleId;

#[derive(Debug)]
pub struct ResolvedModule {
    pub id: ModuleId,
    pub source: String,
    pub path: PathBuf,
    pub ast: ast::Module,
    pub imports: Vec<ImportEntry>,
    pub exports: Vec<ExportEntry>,
    pub dynamic_imports: Vec<String>,
    pub is_cjs: bool,
}

#[derive(Debug, Clone)]
pub struct ImportEntry {
    pub specifier: String,
    pub names: Vec<(String, String)>,
    pub source_span: Span,
}

#[derive(Debug, Clone)]
pub enum ExportEntry {
    Named { local: String, exported: String },
    Default { local: String },
    All { source: String },
    NamedReExport {
        local: String,
        exported: String,
        source: String,
    },
    Declaration { name: String },
}
