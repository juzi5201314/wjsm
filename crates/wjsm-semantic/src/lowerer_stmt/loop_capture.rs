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

    /// 循环体内「按迭代实例化且被嵌套函数捕获」的词法名集合：体内（含嵌套
    /// 块 / switch / try-catch / 嵌套循环头，不穿函数边界与嵌套循环体）声明
    /// 的 let/const/class/using/catch 参数，与体内嵌套函数引用名求交。
    /// 非空时循环须建按迭代 env 帧，这些绑定经帧 env 读写，闭包每轮捕获
    /// 独立实例（ES §14.7.4.3 CreatePerIterationEnvironment 的块级推广）。
    /// 嵌套循环体的声明由内层循环自建帧，故不计入；嵌套循环头的 let/const
    /// 每次进入内层循环语句重建，归属本（外层）帧。
    pub(crate) fn loop_body_per_iteration_capture_names(
        body: &swc_ast::Stmt,
    ) -> HashSet<String> {
        let references = nested_function_references(body);
        if references.is_empty() {
            return HashSet::new();
        }
        let mut declared = HashSet::new();
        collect_per_iteration_lexical_names(body, &mut declared);
        references
            .intersection(&declared)
            .cloned()
            .collect()
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

    /// while / do-while：循环体存在按迭代词法捕获时创建仅承载体绑定的
    /// env 帧（无头部绑定）。`block` 就地推进到帧变量准备完成后的延续块；
    /// env 对象由调用方在每轮体入口经 initialize_empty_iteration_env 新建。
    pub(crate) fn prepare_loop_body_iteration_frame(
        &mut self,
        block: &mut BasicBlockId,
        body: &swc_ast::Stmt,
    ) -> Result<Option<IterationEnvFrame>, LoweringError> {
        let body_capture_names = Self::loop_body_per_iteration_capture_names(body);
        if body_capture_names.is_empty() {
            return Ok(None);
        }
        let (continuation, mut frame) = self.prepare_iteration_env(*block, Vec::new())?;
        *block = continuation;
        frame.body_scope_watermark = Some(self.scopes.scope_count());
        frame.body_capture_names = body_capture_names;
        Ok(Some(frame))
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

/// 收集语句子树内按迭代实例化的词法声明名：块 / if / labeled / with /
/// switch / try 逐层递归；嵌套循环只收头部 let/const（每次进入内层循环
/// 语句重建，归属外层帧），其体由内层循环自建帧；函数 / 类体是独立函数
/// 上下文，不越界。var 与块内函数声明按函数作用域提升，不属按迭代绑定。
fn collect_per_iteration_lexical_names(stmt: &swc_ast::Stmt, names: &mut HashSet<String>) {
    match stmt {
        swc_ast::Stmt::Decl(decl) => collect_lexical_decl_names(decl, names),
        swc_ast::Stmt::Block(block) => {
            for stmt in &block.stmts {
                collect_per_iteration_lexical_names(stmt, names);
            }
        }
        swc_ast::Stmt::If(if_stmt) => {
            collect_per_iteration_lexical_names(&if_stmt.cons, names);
            if let Some(alt) = &if_stmt.alt {
                collect_per_iteration_lexical_names(alt, names);
            }
        }
        swc_ast::Stmt::Labeled(labeled) => {
            collect_per_iteration_lexical_names(&labeled.body, names);
        }
        swc_ast::Stmt::With(with_stmt) => {
            collect_per_iteration_lexical_names(&with_stmt.body, names);
        }
        swc_ast::Stmt::Switch(switch_stmt) => {
            for case in &switch_stmt.cases {
                for stmt in &case.cons {
                    collect_per_iteration_lexical_names(stmt, names);
                }
            }
        }
        swc_ast::Stmt::Try(try_stmt) => {
            for stmt in &try_stmt.block.stmts {
                collect_per_iteration_lexical_names(stmt, names);
            }
            if let Some(handler) = &try_stmt.handler {
                if let Some(param) = &handler.param {
                    collect_pattern_names(param, names);
                }
                for stmt in &handler.body.stmts {
                    collect_per_iteration_lexical_names(stmt, names);
                }
            }
            if let Some(finalizer) = &try_stmt.finalizer {
                for stmt in &finalizer.stmts {
                    collect_per_iteration_lexical_names(stmt, names);
                }
            }
        }
        swc_ast::Stmt::For(for_stmt) => {
            if let Some(swc_ast::VarDeclOrExpr::VarDecl(decl)) = &for_stmt.init
                && decl.kind != swc_ast::VarDeclKind::Var
            {
                for declarator in &decl.decls {
                    collect_pattern_names(&declarator.name, names);
                }
            }
        }
        swc_ast::Stmt::ForIn(for_in) => collect_for_head_lexical_names(&for_in.left, names),
        swc_ast::Stmt::ForOf(for_of) => collect_for_head_lexical_names(&for_of.left, names),
        _ => {}
    }
}

fn collect_lexical_decl_names(decl: &swc_ast::Decl, names: &mut HashSet<String>) {
    match decl {
        swc_ast::Decl::Var(var_decl) if var_decl.kind != swc_ast::VarDeclKind::Var => {
            for declarator in &var_decl.decls {
                collect_pattern_names(&declarator.name, names);
            }
        }
        swc_ast::Decl::Using(using_decl) => {
            for declarator in &using_decl.decls {
                collect_pattern_names(&declarator.name, names);
            }
        }
        swc_ast::Decl::Class(class_decl) => {
            names.insert(class_decl.ident.sym.to_string());
        }
        // var / 函数声明按函数作用域提升；TS enum/namespace 保持 `$0.*`
        // 槽合并降级模型，不参与按迭代 env。
        _ => {}
    }
}

fn collect_for_head_lexical_names(head: &swc_ast::ForHead, names: &mut HashSet<String>) {
    if let swc_ast::ForHead::VarDecl(decl) = head
        && decl.kind != swc_ast::VarDeclKind::Var
    {
        for declarator in &decl.decls {
            collect_pattern_names(&declarator.name, names);
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
