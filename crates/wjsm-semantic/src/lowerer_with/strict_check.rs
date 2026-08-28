//! 严格模式代码的语法级 early error 校验。
//!
//! 覆盖两类规则：WithStatement（§14.11.1）与 eval/arguments 作为赋值目标
//! （§13.1.3 AssignmentTargetType：strict 代码中 IdentifierReference 为
//! "eval" 或 "arguments" 时 AssignmentTargetType 非 simple，赋值 /
//! update / 解构 / for-in/of 头均为 SyntaxError）。
//!
//! 严格模式是语法上下文属性：模块/eval 级指令、函数体 `"use strict"` 指令
//! 序言、类体（含构造器 / 方法 / 静态块 / 字段初始化器）内的代码都属于
//! 严格模式代码。降级期的 `strict_mode` 只有模块/eval 粒度，无法覆盖函数
//! 级与类体，故在降级前做一次 AST 遍历（模式同 `PrivateNameValidator`）。

use swc_core::ecma::visit::{Visit, VisitWith};

use super::*;

/// V8 同口径的 eval/arguments 赋值目标错误消息。
const RESTRICTED_TARGET_MESSAGE: &str = "Unexpected eval or arguments in strict mode";

struct StrictCodeValidator {
    strict: bool,
    error: Option<(Span, &'static str)>,
}

/// 函数体指令序言中是否含 `"use strict"`（遇到首个非指令语句即停）。
pub(crate) fn stmts_have_use_strict(stmts: &[swc_ast::Stmt]) -> bool {
    for stmt in stmts {
        let swc_ast::Stmt::Expr(expr_stmt) = stmt else {
            break;
        };
        let swc_ast::Expr::Lit(swc_ast::Lit::Str(string)) = expr_stmt.expr.as_ref() else {
            break;
        };
        if string.value.as_str() == Some("use strict") {
            return true;
        }
    }
    false
}

/// 标识符是否为严格模式禁止的赋值目标名（§13.1.3）。
fn is_restricted_target_name(name: &str) -> bool {
    name == "eval" || name == "arguments"
}

/// 解构赋值 Pattern 中承接赋值的标识符叶子是否命中 eval/arguments。
/// 只看真正被写入的位置：默认值表达式、计算键等由常规遍历继续下探。
fn pat_binds_restricted_name(pat: &swc_ast::Pat) -> Option<Span> {
    match pat {
        swc_ast::Pat::Ident(ident) => {
            is_restricted_target_name(ident.id.sym.as_ref()).then_some(ident.id.span)
        }
        swc_ast::Pat::Expr(expr) => match expr.as_ref() {
            swc_ast::Expr::Ident(ident) => {
                is_restricted_target_name(ident.sym.as_ref()).then_some(ident.span)
            }
            _ => None,
        },
        swc_ast::Pat::Array(array) => array
            .elems
            .iter()
            .flatten()
            .find_map(pat_binds_restricted_name),
        swc_ast::Pat::Rest(rest) => pat_binds_restricted_name(&rest.arg),
        swc_ast::Pat::Assign(assign) => pat_binds_restricted_name(&assign.left),
        swc_ast::Pat::Object(object) => object.props.iter().find_map(|prop| match prop {
            swc_ast::ObjectPatProp::KeyValue(key_value) => {
                pat_binds_restricted_name(&key_value.value)
            }
            swc_ast::ObjectPatProp::Assign(assign) => {
                is_restricted_target_name(assign.key.id.sym.as_ref()).then_some(assign.key.id.span)
            }
            swc_ast::ObjectPatProp::Rest(rest) => pat_binds_restricted_name(&rest.arg),
        }),
        swc_ast::Pat::Invalid(_) => None,
    }
}

impl StrictCodeValidator {
    fn record(&mut self, span: Span, message: &'static str) {
        if self.error.is_none() {
            self.error = Some((span, message));
        }
    }

    /// strict 代码中赋值目标命中 eval/arguments 时登记 early error。
    fn check_assign_target(&mut self, target: &swc_ast::AssignTarget) {
        if !self.strict {
            return;
        }
        match target {
            swc_ast::AssignTarget::Simple(swc_ast::SimpleAssignTarget::Ident(ident)) => {
                if is_restricted_target_name(ident.id.sym.as_ref()) {
                    self.record(ident.id.span, RESTRICTED_TARGET_MESSAGE);
                }
            }
            swc_ast::AssignTarget::Simple(_) => {}
            swc_ast::AssignTarget::Pat(pat) => {
                let ir_pat = swc_ast::Pat::from(pat.clone());
                if let Some(span) = pat_binds_restricted_name(&ir_pat) {
                    self.record(span, RESTRICTED_TARGET_MESSAGE);
                }
            }
        }
    }
}

impl Visit for StrictCodeValidator {
    fn visit_with_stmt(&mut self, stmt: &swc_ast::WithStmt) {
        if self.strict {
            self.record(stmt.span, "Strict mode code may not include a with statement");
        }
        stmt.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &swc_ast::AssignExpr) {
        self.check_assign_target(&assign.left);
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_ast::UpdateExpr) {
        if self.strict
            && let swc_ast::Expr::Ident(ident) = update.arg.as_ref()
            && is_restricted_target_name(ident.sym.as_ref())
        {
            self.record(ident.span, RESTRICTED_TARGET_MESSAGE);
        }
        update.visit_children_with(self);
    }

    fn visit_for_in_stmt(&mut self, stmt: &swc_ast::ForInStmt) {
        if self.strict
            && let swc_ast::ForHead::Pat(pat) = &stmt.left
            && let Some(span) = pat_binds_restricted_name(pat)
        {
            self.record(span, RESTRICTED_TARGET_MESSAGE);
        }
        stmt.visit_children_with(self);
    }

    fn visit_for_of_stmt(&mut self, stmt: &swc_ast::ForOfStmt) {
        if self.strict
            && let swc_ast::ForHead::Pat(pat) = &stmt.left
            && let Some(span) = pat_binds_restricted_name(pat)
        {
            self.record(span, RESTRICTED_TARGET_MESSAGE);
        }
        stmt.visit_children_with(self);
    }

    fn visit_function(&mut self, function: &swc_ast::Function) {
        let outer = self.strict;
        self.strict = outer
            || function
                .body
                .as_ref()
                .is_some_and(|body| stmts_have_use_strict(&body.stmts));
        function.visit_children_with(self);
        self.strict = outer;
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_ast::ArrowExpr) {
        let outer = self.strict;
        if let swc_ast::BlockStmtOrExpr::BlockStmt(body) = arrow.body.as_ref() {
            self.strict = outer || stmts_have_use_strict(&body.stmts);
        }
        arrow.visit_children_with(self);
        self.strict = outer;
    }

    fn visit_class(&mut self, class: &swc_ast::Class) {
        // ClassBody 全体（构造器、方法、静态块、字段初始化器）恒为严格代码。
        let outer = self.strict;
        self.strict = true;
        class.visit_children_with(self);
        self.strict = outer;
    }
}

/// 对整棵模块 AST 校验严格模式代码的语法级 early error（with 语句、
/// eval/arguments 赋值目标），返回首个违例的源区间与消息（调用方经
/// `Lowerer::error` 生成携带源码上下文的诊断）。
/// `base_strict` 为模块/eval 级严格性（含 direct eval 继承的调用方严格位）。
pub(crate) fn find_strict_code_early_error(
    module: &swc_ast::Module,
    base_strict: bool,
) -> Option<(Span, &'static str)> {
    let mut validator = StrictCodeValidator {
        strict: base_strict,
        error: None,
    };
    module.visit_with(&mut validator);
    validator.error
}
