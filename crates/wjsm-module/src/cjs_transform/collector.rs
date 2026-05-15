use std::collections::{BTreeMap, HashSet};

use swc_core::ecma::ast;
use swc_core::ecma::visit::{Visit, VisitWith};

use super::helpers::extract_require_specifier;

pub(super) struct RequireCollector {
    pub(super) require_map: BTreeMap<String, String>,
    pub(super) next_req_id: u32,
    pub(super) direct_imports: HashSet<String>,
}

impl Visit for RequireCollector {
    fn visit_var_decl(&mut self, n: &ast::VarDecl) {
        for decl in &n.decls {
            if let Some(init) = &decl.init {
                if let ast::Expr::Call(call) = init.as_ref() {
                    if let Some(specifier) = extract_require_specifier(call) {
                        if let ast::Pat::Ident(binding) = &decl.name {
                            let local_name = binding.id.sym.to_string();
                            if !local_name.starts_with("__cjs_req_") {
                                if !self.require_map.contains_key(&specifier) {
                                    self.require_map.insert(specifier, local_name.clone());
                                    self.direct_imports.insert(local_name);
                                }
                                continue;
                            }
                        }
                    }
                }
            }
        }
        n.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, n: &ast::CallExpr) {
        if let Some(specifier) = extract_require_specifier(n) {
            if !self.require_map.contains_key(&specifier) {
                let local_name = format!("__cjs_req_{}", self.next_req_id);
                self.next_req_id += 1;
                self.require_map.insert(specifier, local_name);
            }
            return;
        }
        n.visit_children_with(self);
    }
}
