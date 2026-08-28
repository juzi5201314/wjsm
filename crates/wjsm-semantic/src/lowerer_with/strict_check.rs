//! WithStatement 严格模式 early error（§14.11.1）的语法级校验。
//!
//! 严格模式是语法上下文属性：模块/eval 级指令、函数体 `"use strict"` 指令
//! 序言、类体（含构造器 / 方法 / 静态块 / 字段初始化器）内的代码都属于
//! 严格模式代码，其中出现 WithStatement 一律为 SyntaxError。降级期的
//! `strict_mode` 只有模块/eval 粒度，无法覆盖函数级与类体，故在降级前
//! 做一次 AST 遍历（模式同 `PrivateNameValidator`）。

use swc_core::ecma::visit::{Visit, VisitWith};

use super::*;

struct WithStrictValidator {
    strict: bool,
    error: Option<Span>,
}

/// 函数体指令序言中是否含 `"use strict"`（遇到首个非指令语句即停）。
fn stmts_have_use_strict(stmts: &[swc_ast::Stmt]) -> bool {
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

impl Visit for WithStrictValidator {
    fn visit_with_stmt(&mut self, stmt: &swc_ast::WithStmt) {
        if self.error.is_none() && self.strict {
            self.error = Some(stmt.span);
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

/// 对整棵模块 AST 校验严格模式代码不含 with 语句，返回首个违例的源区间
/// （调用方经 `Lowerer::error` 生成携带源码上下文的诊断）。
/// `base_strict` 为模块/eval 级严格性（含 direct eval 继承的调用方严格位）。
pub(crate) fn find_with_in_strict_code(
    module: &swc_ast::Module,
    base_strict: bool,
) -> Option<Span> {
    let mut validator = WithStrictValidator {
        strict: base_strict,
        error: None,
    };
    module.visit_with(&mut validator);
    validator.error
}
