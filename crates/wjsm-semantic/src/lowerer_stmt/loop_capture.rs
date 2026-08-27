use super::*;

use std::collections::HashSet;

use swc_core::ecma::ast as swc_ast;
use swc_core::ecma::visit::{Visit, VisitWith};

/// 收集循环体内嵌套函数对外层名称的静态引用。
///
/// 参数绑定会在对应函数边界内遮蔽同名外层变量；声明位置本身不算引用。
/// 局部声明造成的额外命中只会提前共享外层绑定，不改变名称解析结果。
pub(super) fn nested_function_references(stmt: &swc_ast::Stmt) -> HashSet<String> {
    let mut scan = NestedFunctionReferenceScan::default();
    stmt.visit_with(&mut scan);

    scan.references
}

impl Lowerer {
    pub(crate) fn loop_body_captured_bindings(&self, body: &swc_ast::Stmt) -> Vec<CapturedBinding> {
        let mut bindings = nested_function_references(body)
            .into_iter()
            .filter_map(|name| {
                let scope_id = self.scopes.resolve_scope_id(&name).ok()?;
                let binding = CapturedBinding::new(name, scope_id);
                self.binding_belongs_to_current_function(&binding)
                    .then_some(binding)
            })
            .collect::<Vec<_>>();
        bindings.sort_by_key(CapturedBinding::env_key);
        bindings
    }

    pub(crate) fn loop_head_iteration_bindings(
        &self,
        declaration: &swc_ast::VarDecl,
        captured: &[CapturedBinding],
        include_const: bool,
    ) -> Result<Vec<CapturedBinding>, LoweringError> {
        let is_iteration_declaration = declaration.kind == swc_ast::VarDeclKind::Let
            || (include_const && declaration.kind == swc_ast::VarDeclKind::Const);
        if !is_iteration_declaration {
            return Ok(Vec::new());
        }

        let mut names = Vec::new();
        for declarator in &declaration.decls {
            Self::extract_pat_bindings(std::slice::from_ref(&declarator.name), &mut names);
        }
        let head_is_captured = captured.iter().any(|binding| names.contains(&binding.name));
        if !head_is_captured {
            return Ok(Vec::new());
        }

        names
            .into_iter()
            .map(|name| {
                let scope_id = self
                    .scopes
                    .resolve_scope_id(&name)
                    .map_err(|message| self.error(declaration.span, message))?;
                Ok(CapturedBinding::new(name, scope_id))
            })
            .collect()
    }

    fn stable_loop_captures(
        &self,
        captured: &[CapturedBinding],
        iteration_bindings: &[CapturedBinding],
    ) -> Vec<CapturedBinding> {
        captured
            .iter()
            .filter(|binding| {
                !iteration_bindings.contains(binding)
                    && self.iteration_env_for_binding(binding).is_none()
            })
            .cloned()
            .collect()
    }

    pub(crate) fn mark_stable_loop_captures(
        &mut self,
        captured: &[CapturedBinding],
        iteration_bindings: &[CapturedBinding],
    ) {
        let stable = self.stable_loop_captures(captured, iteration_bindings);
        self.shared_binding_names_stack
            .last_mut()
            .expect("shared binding names stack underflow")
            .extend(stable);
    }

    pub(crate) fn initialize_stable_loop_captures(
        &mut self,
        block: BasicBlockId,
        captured: &[CapturedBinding],
        iteration_bindings: &[CapturedBinding],
    ) -> Result<BasicBlockId, LoweringError> {
        let stable = self.stable_loop_captures(captured, iteration_bindings);
        if stable.is_empty() {
            return Ok(block);
        }
        let _ = self.ensure_shared_env(block, &stable, DUMMY_SP)?;
        Ok(self.resolve_store_block(block))
    }
}

#[derive(Default)]
struct NestedFunctionReferenceScan {
    function_depth: usize,
    parameter_scopes: Vec<HashSet<String>>,
    references: HashSet<String>,
}

impl NestedFunctionReferenceScan {
    fn visit_function_boundary(
        &mut self,
        params: impl IntoIterator<Item = swc_ast::Pat>,
        visit: impl FnOnce(&mut Self),
    ) {
        let mut parameter_names = HashSet::new();
        for param in params {
            collect_pattern_names(&param, &mut parameter_names);
        }
        self.function_depth += 1;
        self.parameter_scopes.push(parameter_names);
        visit(self);
        self.parameter_scopes.pop();
        self.function_depth -= 1;
    }

    fn parameter_shadows(&self, name: &str) -> bool {
        self.parameter_scopes
            .iter()
            .rev()
            .any(|scope| scope.contains(name))
    }
}

impl Visit for NestedFunctionReferenceScan {
    fn visit_function(&mut self, function: &swc_ast::Function) {
        let params = function.params.iter().map(|param| param.pat.clone());
        self.visit_function_boundary(params, |scan| function.visit_children_with(scan));
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_ast::ArrowExpr) {
        self.visit_function_boundary(arrow.params.iter().cloned(), |scan| {
            arrow.visit_children_with(scan)
        });
    }

    fn visit_ident(&mut self, ident: &swc_ast::Ident) {
        let name = ident.sym.as_ref();
        if self.function_depth > 0 && !self.parameter_shadows(name) {
            self.references.insert(name.to_owned());
        }
    }

    fn visit_class(&mut self, class: &swc_ast::Class) {
        // 类的延迟执行部位（构造器体、实例字段初始化器、方法体）在迭代结束后
        // 仍会运行，等同嵌套函数捕获。计算键 / 静态初始化器在类定义期立即求值，
        // 一并按边界处理只会多标记共享绑定，读到的仍是当前迭代值（见顶部注释）。
        self.visit_function_boundary(std::iter::empty(), |scan| class.visit_children_with(scan));
    }

    fn visit_binding_ident(&mut self, _: &swc_ast::BindingIdent) {}

    fn visit_member_expr(&mut self, member: &swc_ast::MemberExpr) {
        member.obj.visit_with(self);
        if let swc_ast::MemberProp::Computed(computed) = &member.prop {
            computed.expr.visit_with(self);
        }
    }
}

fn collect_pattern_names(pattern: &swc_ast::Pat, names: &mut HashSet<String>) {
    match pattern {
        swc_ast::Pat::Ident(binding) => {
            names.insert(binding.id.sym.to_string());
        }
        swc_ast::Pat::Array(array) => {
            for element in array.elems.iter().flatten() {
                collect_pattern_names(element, names);
            }
        }
        swc_ast::Pat::Object(object) => {
            for property in &object.props {
                match property {
                    swc_ast::ObjectPatProp::KeyValue(property) => {
                        collect_pattern_names(&property.value, names);
                    }
                    swc_ast::ObjectPatProp::Assign(property) => {
                        names.insert(property.key.id.sym.to_string());
                    }
                    swc_ast::ObjectPatProp::Rest(rest) => {
                        collect_pattern_names(&rest.arg, names);
                    }
                }
            }
        }
        swc_ast::Pat::Rest(rest) => collect_pattern_names(&rest.arg, names),
        swc_ast::Pat::Assign(assign) => collect_pattern_names(&assign.left, names),
        swc_ast::Pat::Expr(_) | swc_ast::Pat::Invalid(_) => {}
    }
}
