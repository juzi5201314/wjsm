// wjsm-module: ES Module / CommonJS bundling support
// 将多个模块编译为单一 WASM 二进制

mod bundler;
pub mod cjs_transform;
mod graph;
mod resolver;
mod semantic;
use swc_core::ecma::ast;

pub use bundler::ModuleBundler;
pub use graph::{ModuleGraph, ModuleId};
pub use resolver::{ExportEntry, ImportEntry, ModuleResolver, ResolvedModule};
pub use semantic::{ModuleLinkResult, analyze_module_links};

use anyhow::Result;
use std::path::Path;

/// Bundle entry module and all its dependencies into a single WASM binary
pub fn bundle(entry: &str, root_path: &Path) -> Result<Vec<u8>> {
    let bundler = ModuleBundler::new(root_path)?;
    bundler.bundle(entry)
}

// ── 模块类型检测 ───────────────────────────────────────────────────

/// 检测 AST 是否包含 ES Module 语法（import/export 声明）
pub fn is_es_module(module: &ast::Module) -> bool {
    module.body.iter().any(|item| {
        // 静态 import/export 声明
        if matches!(
            item,
            ast::ModuleItem::ModuleDecl(
                ast::ModuleDecl::Import(_)
                    | ast::ModuleDecl::ExportDecl(_)
                    | ast::ModuleDecl::ExportNamed(_)
                    | ast::ModuleDecl::ExportDefaultDecl(_)
                    | ast::ModuleDecl::ExportDefaultExpr(_)
                    | ast::ModuleDecl::ExportAll(_)
            )
        ) {
            return true;
        }
        // 动态 import() 调用也表明是 ES Module
        if let ast::ModuleItem::Stmt(stmt) = item {
            return stmt_has_dynamic_import(stmt);
        }
        false
    })
}

/// 检测 AST 是否包含 CommonJS 语法（require/exports/module.exports）
/// 代理到 cjs_transform::is_commonjs_module
pub fn is_commonjs_module(module: &ast::Module) -> bool {
    cjs_transform::is_commonjs_module(module)
}


/// 递归检测声明中是否包含动态 import() 调用
fn decl_has_dynamic_import(decl: &ast::Decl) -> bool {
    match decl {
        ast::Decl::Fn(fn_decl) => fn_decl
            .function
            .body
            .as_ref()
            .map_or(false, |body| body.stmts.iter().any(stmt_has_dynamic_import)),
        ast::Decl::Class(class_decl) => class_decl.class.body.iter().any(|member| match member {
            ast::ClassMember::Method(method) => method
                .function
                .body
                .as_ref()
                .map_or(false, |body| body.stmts.iter().any(stmt_has_dynamic_import)),
            ast::ClassMember::Constructor(ctor) => ctor
                .body
                .as_ref()
                .map_or(false, |body| body.stmts.iter().any(stmt_has_dynamic_import)),
            _ => false,
        }),
        ast::Decl::Var(var_decl) => var_decl
            .decls
            .iter()
            .any(|d| d.init.as_ref().map_or(false, |e| expr_has_dynamic_import(e))),
        _ => false,
    }
}

/// 递归检测语句中是否包含动态 import() 调用
fn stmt_has_dynamic_import(stmt: &ast::Stmt) -> bool {
    match stmt {
        ast::Stmt::Expr(expr_stmt) => expr_has_dynamic_import(&expr_stmt.expr),
        ast::Stmt::Block(block) => block.stmts.iter().any(stmt_has_dynamic_import),
        ast::Stmt::If(if_stmt) => {
            expr_has_dynamic_import(&if_stmt.test)
                || stmt_has_dynamic_import(&if_stmt.cons)
                || if_stmt.alt.as_ref().map_or(false, |alt| stmt_has_dynamic_import(alt))
        }
        ast::Stmt::While(while_stmt) => {
            expr_has_dynamic_import(&while_stmt.test) || stmt_has_dynamic_import(&while_stmt.body)
        }
        ast::Stmt::For(for_stmt) => {
            for_stmt.init.as_ref().map_or(false, |init| match init {
                ast::VarDeclOrExpr::VarDecl(decl) => decl.decls.iter().any(|d| {
                    d.init.as_ref().map_or(false, |e| expr_has_dynamic_import(e))
                }),
                ast::VarDeclOrExpr::Expr(e) => expr_has_dynamic_import(e),
            }) || for_stmt.test.as_ref().map_or(false, |e| expr_has_dynamic_import(e))
            || for_stmt.update.as_ref().map_or(false, |e| expr_has_dynamic_import(e))
            || stmt_has_dynamic_import(&for_stmt.body)
        }
        ast::Stmt::Return(ret) => ret.arg.as_ref().map_or(false, |e| expr_has_dynamic_import(e)),
        ast::Stmt::Decl(decl) => decl_has_dynamic_import(decl),
        ast::Stmt::Throw(throw) => expr_has_dynamic_import(&throw.arg),
        ast::Stmt::Try(try_stmt) => {
            try_stmt.block.stmts.iter().any(stmt_has_dynamic_import)
                || try_stmt.handler.as_ref().map_or(false, |h| {
                    h.body.stmts.iter().any(stmt_has_dynamic_import)
                })
                || try_stmt.finalizer.as_ref().map_or(false, |f| {
                    f.stmts.iter().any(stmt_has_dynamic_import)
                })
        }
        ast::Stmt::Switch(switch) => {
            expr_has_dynamic_import(&switch.discriminant)
                || switch.cases.iter().any(|c| {
                    c.test.as_ref().map_or(false, |e| expr_has_dynamic_import(e))
                        || c.cons.iter().any(stmt_has_dynamic_import)
                })
        }
        ast::Stmt::Labeled(label) => stmt_has_dynamic_import(&label.body),
        _ => false,
    }
}

/// 递归检测表达式中是否包含动态 import() 调用
fn expr_has_dynamic_import(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Call(call) => {
            matches!(call.callee, ast::Callee::Import(_))
                || matches!(&call.callee, ast::Callee::Expr(e) if expr_has_dynamic_import(e))
                || call.args.iter().any(|a| expr_has_dynamic_import(&a.expr))
        }
        ast::Expr::Member(member) => {
            expr_has_dynamic_import(&member.obj)
                || match &member.prop {
                    ast::MemberProp::Computed(c) => expr_has_dynamic_import(&c.expr),
                    _ => false,
                }
        }
        ast::Expr::Assign(assign) => {
            expr_has_dynamic_import(&assign.right)
        }
        ast::Expr::Bin(bin) => {
            expr_has_dynamic_import(&bin.left) || expr_has_dynamic_import(&bin.right)
        }
        ast::Expr::Cond(cond) => {
            expr_has_dynamic_import(&cond.test)
                || expr_has_dynamic_import(&cond.cons)
                || expr_has_dynamic_import(&cond.alt)
        }
        ast::Expr::Unary(unary) => expr_has_dynamic_import(&unary.arg),
        ast::Expr::Update(update) => expr_has_dynamic_import(&update.arg),
        ast::Expr::Seq(seq) => seq.exprs.iter().any(|e| expr_has_dynamic_import(e)),
        ast::Expr::Paren(paren) => expr_has_dynamic_import(&paren.expr),
        ast::Expr::Arrow(arrow) => match &*arrow.body {
            ast::BlockStmtOrExpr::BlockStmt(block) => {
                block.stmts.iter().any(stmt_has_dynamic_import)
            }
            ast::BlockStmtOrExpr::Expr(e) => expr_has_dynamic_import(e),
        },
        ast::Expr::Fn(fn_expr) => {
            fn_expr.function.as_ref().body.as_ref().map_or(false, |body| {
                body.stmts.iter().any(stmt_has_dynamic_import)
            })
        }
        _ => false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn public_bundle_function_works() {
        let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("fixtures/modules/simple");

        if !fixtures_dir.exists() {
            return;
        }

        let result = bundle("./main.js", &fixtures_dir);
        assert!(
            result.is_ok(),
            "public bundle should succeed: {:?}",
            result.err()
        );
        let wasm_bytes = result.unwrap();
        assert!(
            !wasm_bytes.is_empty(),
            "should produce non-empty WASM output"
        );
        assert!(
            wasm_bytes.starts_with(b"\x00asm"),
            "output should be valid WASM binary"
        );
    }
}
