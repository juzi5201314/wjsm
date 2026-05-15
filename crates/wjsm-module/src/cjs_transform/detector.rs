use swc_core::ecma::ast;
use swc_core::ecma::visit::{Visit, VisitWith};

use super::helpers::{
    extract_require_specifier, is_exports_ident, is_exports_member, is_module_exports_member,
    is_module_exports_member_no_prop,
};

pub(super) struct CjsDetector {
    pub(super) found: bool,
}

impl Visit for CjsDetector {
    fn visit_call_expr(&mut self, n: &ast::CallExpr) {
        if self.found {
            return;
        }
        if extract_require_specifier(n).is_some() {
            self.found = true;
            return;
        }
        n.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, n: &ast::MemberExpr) {
        if self.found {
            return;
        }
        if is_module_exports_member(&ast::Expr::Member(n.clone()))
            || is_exports_member(&ast::Expr::Member(n.clone()))
        {
            self.found = true;
            return;
        }
        n.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, n: &ast::AssignExpr) {
        if self.found {
            return;
        }
        if let ast::AssignTarget::Simple(simple) = &n.left {
            if let ast::SimpleAssignTarget::Member(member) = simple {
                if is_module_exports_member(&member.obj) || is_exports_ident(&member.obj) {
                    self.found = true;
                    return;
                }
                if is_module_exports_member_no_prop(&member.obj, &member.prop) {
                    self.found = true;
                    return;
                }
            }
        }
        n.visit_children_with(self);
    }
}
