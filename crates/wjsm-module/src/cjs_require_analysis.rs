use std::collections::{BTreeMap, HashSet};

use swc_core::common::Span;
use swc_core::ecma::ast;
use swc_core::ecma::visit::{Visit, VisitWith};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequireSiteKind {
    HoistableStatic { specifier: String },
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RequireSiteKey {
    lo: u32,
    hi: u32,
}

impl RequireSiteKey {
    pub(crate) fn from_span(span: Span) -> Self {
        Self {
            lo: span.lo.0,
            hi: span.hi.0,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct RequireAnalysis {
    pub(crate) hoistable: BTreeMap<String, String>,
    pub(crate) direct_imports: HashSet<String>,
    pub(crate) hoistable_sites: HashSet<RequireSiteKey>,
    pub(crate) runtime_sites: usize,
}

pub(crate) fn analyze_require_sites(module: &ast::Module) -> RequireAnalysis {
    let mut analyzer = RequireAnalyzer {
        analysis: RequireAnalysis::default(),
        next_req_id: 0,
        hoistable_context: true,
    };
    module.visit_with(&mut analyzer);
    analyzer.analysis
}

pub(crate) fn is_require_call(call: &ast::CallExpr) -> bool {
    matches!(
        &call.callee,
        ast::Callee::Expr(expr)
            if matches!(
                expr.as_ref(),
                ast::Expr::Ident(ident) if ident.sym.as_ref() == "require"
            )
    ) && call.args.len() == 1
}

pub(crate) fn extract_require_specifier(call: &ast::CallExpr) -> Option<String> {
    if is_require_call(call) {
        extract_static_module_specifier(&call.args[0].expr)
    } else {
        None
    }
}

/// 从 require()/import() 参数表达式提取静态模块说明符。
pub(crate) fn extract_static_module_specifier(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Lit(ast::Lit::Str(s)) => Some(s.value.to_string_lossy().into_owned()),
        ast::Expr::Tpl(tpl) if tpl.quasis.len() == 1 && tpl.exprs.is_empty() => tpl.quasis[0]
            .cooked
            .as_ref()
            .map(|cooked| cooked.to_string_lossy().into_owned()),
        _ => None,
    }
}

fn should_hoist_static_require(specifier: &str) -> bool {
    // JSON 的 CommonJS require 需要运行时解析与 JSON 解析，不能改写成 ESM import。
    !specifier.ends_with(".json")
}

fn classify_require_call(call: &ast::CallExpr, hoistable_context: bool) -> Option<RequireSiteKind> {
    if !is_require_call(call) {
        return None;
    }
    if hoistable_context
        && let Some(specifier) = extract_static_module_specifier(&call.args[0].expr)
        && should_hoist_static_require(&specifier)
    {
        Some(RequireSiteKind::HoistableStatic { specifier })
    } else {
        Some(RequireSiteKind::Runtime)
    }
}

struct RequireAnalyzer {
    analysis: RequireAnalysis,
    next_req_id: u32,
    hoistable_context: bool,
}

impl RequireAnalyzer {
    fn record_hoistable_static(
        &mut self,
        call: &ast::CallExpr,
        specifier: String,
        direct_local: Option<String>,
    ) {
        self.analysis
            .hoistable_sites
            .insert(RequireSiteKey::from_span(call.span));

        if let Some(local_name) = direct_local
            && !local_name.starts_with("__cjs_req_")
        {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                self.analysis.hoistable.entry(specifier.clone())
            {
                entry.insert(local_name.clone());
                self.analysis.direct_imports.insert(local_name);
            }
            return;
        }

        if !self.analysis.hoistable.contains_key(&specifier) {
            let local_name = format!("__cjs_req_{}", self.next_req_id);
            self.next_req_id += 1;
            self.analysis.hoistable.insert(specifier, local_name);
        }
    }

    fn with_runtime_context(&mut self, visit: impl FnOnce(&mut Self)) {
        let previous = self.hoistable_context;
        self.hoistable_context = false;
        visit(self);
        self.hoistable_context = previous;
    }
}

impl Visit for RequireAnalyzer {
    fn visit_module(&mut self, module: &ast::Module) {
        for item in &module.body {
            if let ast::ModuleItem::Stmt(stmt) = item {
                stmt.visit_with(self);
            }
        }
    }

    fn visit_var_decl(&mut self, var_decl: &ast::VarDecl) {
        if !self.hoistable_context {
            var_decl.visit_children_with(self);
            return;
        }

        for decl in &var_decl.decls {
            if let Some(init) = &decl.init
                && let ast::Expr::Call(call) = init.as_ref()
                && let Some(RequireSiteKind::HoistableStatic { specifier }) =
                    classify_require_call(call, true)
                && let ast::Pat::Ident(binding) = &decl.name
            {
                self.record_hoistable_static(call, specifier, Some(binding.id.sym.to_string()));
            } else {
                decl.visit_with(self);
            }
        }
    }

    fn visit_call_expr(&mut self, call: &ast::CallExpr) {
        match classify_require_call(call, self.hoistable_context) {
            Some(RequireSiteKind::HoistableStatic { specifier }) => {
                self.record_hoistable_static(call, specifier, None);
            }
            Some(RequireSiteKind::Runtime) => {
                self.analysis.runtime_sites += 1;
            }
            None => call.visit_children_with(self),
        }
    }

    fn visit_block_stmt(&mut self, block: &ast::BlockStmt) {
        self.with_runtime_context(|analyzer| block.visit_children_with(analyzer));
    }

    fn visit_if_stmt(&mut self, stmt: &ast::IfStmt) {
        self.with_runtime_context(|analyzer| stmt.visit_children_with(analyzer));
    }

    fn visit_while_stmt(&mut self, stmt: &ast::WhileStmt) {
        self.with_runtime_context(|analyzer| stmt.visit_children_with(analyzer));
    }

    fn visit_do_while_stmt(&mut self, stmt: &ast::DoWhileStmt) {
        self.with_runtime_context(|analyzer| stmt.visit_children_with(analyzer));
    }

    fn visit_for_stmt(&mut self, stmt: &ast::ForStmt) {
        self.with_runtime_context(|analyzer| stmt.visit_children_with(analyzer));
    }

    fn visit_for_in_stmt(&mut self, stmt: &ast::ForInStmt) {
        self.with_runtime_context(|analyzer| stmt.visit_children_with(analyzer));
    }

    fn visit_for_of_stmt(&mut self, stmt: &ast::ForOfStmt) {
        self.with_runtime_context(|analyzer| stmt.visit_children_with(analyzer));
    }

    fn visit_switch_stmt(&mut self, stmt: &ast::SwitchStmt) {
        self.with_runtime_context(|analyzer| stmt.visit_children_with(analyzer));
    }

    fn visit_try_stmt(&mut self, stmt: &ast::TryStmt) {
        self.with_runtime_context(|analyzer| stmt.visit_children_with(analyzer));
    }

    fn visit_with_stmt(&mut self, stmt: &ast::WithStmt) {
        self.with_runtime_context(|analyzer| stmt.visit_children_with(analyzer));
    }

    fn visit_cond_expr(&mut self, expr: &ast::CondExpr) {
        self.with_runtime_context(|analyzer| expr.visit_children_with(analyzer));
    }

    fn visit_bin_expr(&mut self, expr: &ast::BinExpr) {
        if matches!(
            expr.op,
            ast::BinaryOp::LogicalAnd | ast::BinaryOp::LogicalOr | ast::BinaryOp::NullishCoalescing
        ) {
            self.with_runtime_context(|analyzer| expr.visit_children_with(analyzer));
        } else {
            expr.visit_children_with(self);
        }
    }

    fn visit_function(&mut self, function: &ast::Function) {
        self.with_runtime_context(|analyzer| function.visit_children_with(analyzer));
    }

    fn visit_arrow_expr(&mut self, arrow: &ast::ArrowExpr) {
        self.with_runtime_context(|analyzer| arrow.visit_children_with(analyzer));
    }

    fn visit_class(&mut self, class: &ast::Class) {
        self.with_runtime_context(|analyzer| class.visit_children_with(analyzer));
    }
}
