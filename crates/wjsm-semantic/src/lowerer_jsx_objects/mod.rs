use super::*;
use swc_core::ecma::visit::{Visit, VisitWith};

pub(crate) struct LoweredMethodFunction {
    pub(crate) function_id: FunctionId,
    pub(crate) captured: Vec<CapturedBinding>,
}

#[derive(Default)]
struct ObjectMethodHomeUse {
    needs_home: bool,
}

impl Visit for ObjectMethodHomeUse {
    fn visit_super_prop_expr(&mut self, _: &swc_ast::SuperPropExpr) {
        self.needs_home = true;
    }

    fn visit_call_expr(&mut self, call: &swc_ast::CallExpr) {
        if let swc_ast::Callee::Expr(callee) = &call.callee
            && let swc_ast::Expr::Ident(ident) = callee.as_ref()
            && ident.sym.as_ref() == "eval"
        {
            self.needs_home = true;
        }
        call.visit_children_with(self);
    }

    fn visit_function(&mut self, _: &swc_ast::Function) {}

    fn visit_class(&mut self, _: &swc_ast::Class) {}
}

fn block_needs_home_object(block: &swc_ast::BlockStmt) -> bool {
    let mut visitor = ObjectMethodHomeUse::default();
    block.visit_with(&mut visitor);
    visitor.needs_home
}

mod jsx_arrays_members;
mod jsx_elements;
mod jsx_expressions;
mod jsx_objects_methods;
