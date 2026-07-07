// wjsm-module: ES Module / CommonJS bundling support
// 将多个模块编译为单一 WASM 二进制

mod builtin_modules;
mod cjs_require_analysis;
mod bundler;
pub mod cjs_transform;
mod exports;
mod graph;
mod module_format;
mod package_json;
mod resolution_options;
mod resolver;
mod runtime_resolution;
mod semantic;
use swc_core::ecma::ast;

pub use bundler::{ModuleBundler, RuntimeEntryBundle};
pub use graph::{ModuleGraph, ModuleId};
pub use resolution_options::ResolutionOptions;
pub use resolver::{ExportEntry, ImportEntry, ModuleResolver, ResolvedModule};
pub use runtime_resolution::{
    RuntimeModuleFormat, RuntimeModuleKey, RuntimeResolveKind, RuntimeResolvePaths,
    RuntimeResolvedModule, resolve_runtime_paths, resolve_runtime_specifier,
};
pub use semantic::{ModuleLinkResult, analyze_module_links};

use anyhow::{Context, Result};
use std::path::Path;

/// 将入口模块及其依赖 lower 为 IR（不编译 WASM）
pub fn lower_bundle(entry: &Path, root_path: &Path) -> Result<wjsm_ir::Program> {
    lower_bundle_with_options(entry, root_path, ResolutionOptions::default())
}

/// Lowers an entry module with explicit package resolution options.
pub fn lower_bundle_with_options(
    entry: &Path,
    root_path: &Path,
    options: ResolutionOptions,
) -> Result<wjsm_ir::Program> {
    let bundler = ModuleBundler::with_resolution_options(root_path, options)?;
    bundler.lower_bundle(entry)
}

/// Lowers a runtime-loaded entry module and creates a namespace for that entry.
pub fn lower_runtime_entry_bundle_with_options(
    entry: &Path,
    root_path: &Path,
    options: ResolutionOptions,
) -> Result<RuntimeEntryBundle> {
    let bundler = ModuleBundler::with_resolution_options(root_path, options)?;
    bundler.lower_runtime_entry_bundle(entry)
}

/// 解析入口模块 AST（用于 dump-ast 等，会构建依赖图）
pub fn parse_entry_ast(entry: &Path, root_path: &Path) -> Result<swc_core::ecma::ast::Module> {
    parse_entry_ast_with_options(entry, root_path, ResolutionOptions::default())
}

/// Parses an entry module AST with explicit package resolution options.
pub fn parse_entry_ast_with_options(
    entry: &Path,
    root_path: &Path,
    options: ResolutionOptions,
) -> Result<swc_core::ecma::ast::Module> {
    let bundler = ModuleBundler::with_resolution_options(root_path, options)?;
    bundler.parse_entry_ast(entry)
}

/// 将入口模块和按 bundle graph 收集到的所有依赖编译为完整 WASM bytes。
///
/// 返回值可直接交给 runtime 执行；失败时错误会携带 `entry` 和 `root_path`，
/// 方便调用方定位是哪个入口图构建失败。
pub fn bundle(entry: &Path, root_path: &Path) -> Result<Vec<u8>> {
    bundle_with_options(entry, root_path, ResolutionOptions::default())
}

/// Bundles an entry module with explicit package resolution options.
pub fn bundle_with_options(
    entry: &Path,
    root_path: &Path,
    options: ResolutionOptions,
) -> Result<Vec<u8>> {
    let bundler = ModuleBundler::with_resolution_options(root_path, options)
        .with_context(|| format!("create module bundler for root {}", root_path.display()))?;
    bundler.bundle(entry).with_context(|| {
        format!(
            "bundle entry {} from root {}",
            entry.display(),
            root_path.display()
        )
    })
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
            .is_some_and(|body| body.stmts.iter().any(stmt_has_dynamic_import)),
        ast::Decl::Class(class_decl) => class_decl.class.body.iter().any(|member| match member {
            ast::ClassMember::Method(method) => method
                .function
                .body
                .as_ref()
                .is_some_and(|body| body.stmts.iter().any(stmt_has_dynamic_import)),
            ast::ClassMember::Constructor(ctor) => ctor
                .body
                .as_ref()
                .is_some_and(|body| body.stmts.iter().any(stmt_has_dynamic_import)),
            _ => false,
        }),
        ast::Decl::Var(var_decl) => var_decl
            .decls
            .iter()
            .any(|d| d.init.as_ref().is_some_and(|e| expr_has_dynamic_import(e))),
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
                || if_stmt
                    .alt
                    .as_ref()
                    .is_some_and(|alt| stmt_has_dynamic_import(alt))
        }
        ast::Stmt::While(while_stmt) => {
            expr_has_dynamic_import(&while_stmt.test) || stmt_has_dynamic_import(&while_stmt.body)
        }
        ast::Stmt::For(for_stmt) => {
            for_stmt.init.as_ref().is_some_and(|init| match init {
                ast::VarDeclOrExpr::VarDecl(decl) => decl
                    .decls
                    .iter()
                    .any(|d| d.init.as_ref().is_some_and(|e| expr_has_dynamic_import(e))),
                ast::VarDeclOrExpr::Expr(e) => expr_has_dynamic_import(e),
            }) || for_stmt
                .test
                .as_ref()
                .is_some_and(|e| expr_has_dynamic_import(e))
                || for_stmt
                    .update
                    .as_ref()
                    .is_some_and(|e| expr_has_dynamic_import(e))
                || stmt_has_dynamic_import(&for_stmt.body)
        }
        ast::Stmt::Return(ret) => ret.arg.as_ref().is_some_and(|e| expr_has_dynamic_import(e)),
        ast::Stmt::Decl(decl) => decl_has_dynamic_import(decl),
        ast::Stmt::Throw(throw) => expr_has_dynamic_import(&throw.arg),
        ast::Stmt::Try(try_stmt) => {
            try_stmt.block.stmts.iter().any(stmt_has_dynamic_import)
                || try_stmt
                    .handler
                    .as_ref()
                    .is_some_and(|h| h.body.stmts.iter().any(stmt_has_dynamic_import))
                || try_stmt
                    .finalizer
                    .as_ref()
                    .is_some_and(|f| f.stmts.iter().any(stmt_has_dynamic_import))
        }
        ast::Stmt::Switch(switch) => {
            expr_has_dynamic_import(&switch.discriminant)
                || switch.cases.iter().any(|c| {
                    c.test.as_ref().is_some_and(|e| expr_has_dynamic_import(e))
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
        ast::Expr::Assign(assign) => expr_has_dynamic_import(&assign.right),
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
        ast::Expr::Object(obj) => obj.props.iter().any(|prop| match prop {
            ast::PropOrSpread::Prop(p) => match p.as_ref() {
                ast::Prop::KeyValue(kv) => expr_has_dynamic_import(&kv.value),
                ast::Prop::Getter(g) => g
                    .body
                    .as_ref()
                    .is_some_and(|body| body.stmts.iter().any(stmt_has_dynamic_import)),
                ast::Prop::Setter(s) => s
                    .body
                    .as_ref()
                    .is_some_and(|body| body.stmts.iter().any(stmt_has_dynamic_import)),
                ast::Prop::Method(m) => m
                    .function
                    .body
                    .as_ref()
                    .is_some_and(|body| body.stmts.iter().any(stmt_has_dynamic_import)),
                _ => false,
            },
            ast::PropOrSpread::Spread(spread) => expr_has_dynamic_import(&spread.expr),
        }),
        ast::Expr::Array(arr) => arr
            .elems
            .iter()
            .flatten()
            .any(|elem| expr_has_dynamic_import(&elem.expr)),
        ast::Expr::New(new_expr) => {
            expr_has_dynamic_import(&new_expr.callee)
                || new_expr
                    .args
                    .as_ref()
                    .is_some_and(|args| args.iter().any(|a| expr_has_dynamic_import(&a.expr)))
        }
        ast::Expr::Tpl(tpl) => tpl.exprs.iter().any(|e| expr_has_dynamic_import(e)),
        ast::Expr::TaggedTpl(tagged) => {
            expr_has_dynamic_import(&tagged.tag)
                || tagged.tpl.exprs.iter().any(|e| expr_has_dynamic_import(e))
        }
        ast::Expr::Await(await_expr) => expr_has_dynamic_import(&await_expr.arg),
        ast::Expr::Yield(yield_expr) => yield_expr
            .arg
            .as_ref()
            .is_some_and(|e| expr_has_dynamic_import(e)),
        ast::Expr::OptChain(opt_chain) => match opt_chain.base.as_ref() {
            ast::OptChainBase::Member(member) => expr_has_dynamic_import(&member.obj),
            ast::OptChainBase::Call(call) => {
                expr_has_dynamic_import(&call.callee)
                    || call.args.iter().any(|a| expr_has_dynamic_import(&a.expr))
            }
        },
        ast::Expr::Class(class_expr) => class_expr.class.body.iter().any(|member| match member {
            ast::ClassMember::Method(method) => method
                .function
                .body
                .as_ref()
                .is_some_and(|body| body.stmts.iter().any(stmt_has_dynamic_import)),
            ast::ClassMember::Constructor(ctor) => ctor
                .body
                .as_ref()
                .is_some_and(|body| body.stmts.iter().any(stmt_has_dynamic_import)),
            _ => false,
        }),
        ast::Expr::Fn(fn_expr) => fn_expr
            .function
            .body
            .as_ref()
            .is_some_and(|body| body.stmts.iter().any(stmt_has_dynamic_import)),
        _ => false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn parse_module(source: &str) -> ast::Module {
        wjsm_parser::parse_module(source).expect("parse should succeed")
    }

    #[test]
    fn is_es_module_detects_await_dynamic_import_in_export() {
        let module = parse_module("export const mod = await import('./dynamic.js');");
        assert!(is_es_module(&module));
    }

    #[test]
    fn is_es_module_detects_dynamic_import_in_object_literal() {
        let module = parse_module("const x = { m: import('./dynamic.js') };");
        assert!(is_es_module(&module));
    }

    #[test]
    fn public_bundle_function_works() {
        let root =
            std::env::temp_dir().join(format!("wjsm_module_public_bundle_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp project dir should be creatable");
        std::fs::write(root.join("package.json"), r#"{"type":"module"}"#)
            .expect("package should be writable");
        std::fs::write(
            root.join("main.js"),
            "import { value } from './lib.js';\nconsole.log(value);\n",
        )
        .expect("main module should be writable");
        std::fs::write(root.join("lib.js"), "export const value = 42;\n")
            .expect("lib module should be writable");

        let result = bundle(Path::new("main.js"), &root);
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

        let _ = std::fs::remove_dir_all(&root);
    }
}
