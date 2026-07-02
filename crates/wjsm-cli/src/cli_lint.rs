use swc_core::ecma::ast::{BinExpr, BinaryOp, DebuggerStmt, Module};
use swc_core::ecma::visit::{Visit, VisitWith};

pub(crate) struct LintDiagnostic {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}

pub(crate) fn lint_module(module: &Module) -> Vec<LintDiagnostic> {
    let mut collector = LintCollector {
        diagnostics: Vec::new(),
    };
    module.visit_with(&mut collector);
    collector.diagnostics
}

struct LintCollector {
    diagnostics: Vec<LintDiagnostic>,
}

impl Visit for LintCollector {
    fn visit_debugger_stmt(&mut self, _: &DebuggerStmt) {
        self.diagnostics.push(LintDiagnostic {
            code: "debugger-noop",
            message: "`debugger` is a compile-time no-op in wjsm; remove it or replace it with explicit logging in test code",
        });
    }

    fn visit_bin_expr(&mut self, expr: &BinExpr) {
        match expr.op {
            BinaryOp::EqEq => self.diagnostics.push(LintDiagnostic {
                code: "eqeq",
                message: "use `===` instead of `==` to avoid implicit coercion",
            }),
            BinaryOp::NotEq => self.diagnostics.push(LintDiagnostic {
                code: "neqeq",
                message: "use `!==` instead of `!=` to avoid implicit coercion",
            }),
            _ => {}
        }
        expr.visit_children_with(self);
    }
}
