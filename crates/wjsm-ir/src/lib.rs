pub mod value;

use swc_core::ecma::ast as swc_ast;

#[derive(Debug)]
pub struct Program {
    module: swc_ast::Module,
}

impl Program {
    pub fn new(module: swc_ast::Module) -> Self {
        Self { module }
    }

    pub fn module(&self) -> &swc_ast::Module {
        &self.module
    }
}
