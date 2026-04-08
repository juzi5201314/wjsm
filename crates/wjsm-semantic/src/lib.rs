use swc_core::ecma::ast as swc_ast;
use wjsm_ir::Program;

pub fn lower_module(module: swc_ast::Module) -> Program {
    Program::new(module)
}
