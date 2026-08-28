// wjsm-module: ES Module / CommonJS bundling support
// 将多个模块 lower 为单一 semantic IR program

mod builtin_cache;
mod builtin_modules;
mod bundler;
mod cjs_require_analysis;
pub mod cjs_transform;
mod exports;
mod graph;
mod module_format;
mod package_json;
mod resolution_options;
mod resolver;
mod runtime_resolution;
mod semantic;
mod source_store;
mod static_runtime_entries;
use swc_core::ecma::ast;

pub use builtin_modules::builtin_module_names;
pub use bundler::{ModuleBundler, RuntimeEntryBundle, logical_url_from_path, logical_url_path};
pub use graph::{ModuleGraph, ModuleId};
pub use resolution_options::ResolutionOptions;
pub use resolver::{ExportEntry, ImportEntry, ModuleResolver, ResolvedModule};
pub use runtime_resolution::{
    RuntimeModuleFormat, RuntimeModuleKey, RuntimeResolveKind, RuntimeResolvePaths,
    RuntimeResolvedModule, resolve_runtime_paths, resolve_runtime_paths_with_store,
    resolve_runtime_specifier, resolve_runtime_specifier_with_store,
};
pub use semantic::{ModuleLinkResult, analyze_module_links};
pub use source_store::{
    ModuleSourceStore, SNAPSHOT_FILE_URL_PREFIX, SNAPSHOT_VIRTUAL_ROOT, is_snapshot_fs_path,
    snapshot_file_url, snapshot_virtual_path, snapshot_virtual_root,
};
pub use static_runtime_entries::include_static_runtime_entries;

use anyhow::{Context, Result};
use std::path::Path;

/// 将入口模块及其依赖 lower 为 IR（不执行 codegen）
pub fn lower_bundle(entry: &Path, root_path: &Path) -> Result<wjsm_ir::Program> {
    lower_bundle_with_options(entry, root_path, ResolutionOptions::default())
}

/// Lowers an entry module with explicit package resolution options.
pub fn lower_bundle_with_options(
    entry: &Path,
    root_path: &Path,
    options: ResolutionOptions,
) -> Result<wjsm_ir::Program> {
    lower_bundle_with_debug(entry, root_path, options, false)
}

/// 同 [`lower_bundle_with_options`]，可开启语句级 debug 插桩。
pub fn lower_bundle_with_debug(
    entry: &Path,
    root_path: &Path,
    options: ResolutionOptions,
    emit_debug_checks: bool,
) -> Result<wjsm_ir::Program> {
    let bundler = ModuleBundler::with_resolution_options(root_path, options)?
        .with_emit_debug_checks(emit_debug_checks);
    bundler.lower_bundle(entry)
}
/// Lower 入口模块及依赖并保留 portable manifest。
pub fn lower_artifact_input(
    entry: &Path,
    root_path: &Path,
) -> Result<wjsm_artifact_format::ArtifactBuildInput> {
    lower_artifact_input_with_options(entry, root_path, ResolutionOptions::default(), false)
}

/// 使用显式解析与 debug 选项构造 portable artifact 输入。
pub fn lower_artifact_input_with_options(
    entry: &Path,
    root_path: &Path,
    options: ResolutionOptions,
    emit_debug_checks: bool,
) -> Result<wjsm_artifact_format::ArtifactBuildInput> {
    ModuleBundler::with_resolution_options(root_path, options)?
        .with_emit_debug_checks(emit_debug_checks)
        .lower_artifact_input(entry)
}

/// 用显式 store lower portable artifact 输入（打包期 Recording / packed Snapshot）。
pub fn lower_artifact_input_with_store(
    entry: &Path,
    store: ModuleSourceStore,
    options: ResolutionOptions,
    emit_debug_checks: bool,
) -> Result<wjsm_artifact_format::ArtifactBuildInput> {
    ModuleBundler::with_store(store, options)?
        .with_emit_debug_checks(emit_debug_checks)
        .lower_artifact_input(entry)
}

/// 同 [`lower_bundle_with_debug`]，但 builtin 依赖闭包走独立 lower + 磁盘缓存
/// （`${WJSM_CACHE_DIR}/builtin_ir`，issue #344）。`WJSM_CACHE_DIR` 未设置时构建段但不落盘；
/// `WJSM_NO_BUILTIN_CACHE` 非空时整体退化为 [`lower_bundle_with_debug`]。
pub fn lower_bundle_cached(entry: &Path, root_path: &Path) -> Result<wjsm_ir::Program> {
    lower_bundle_cached_with_options(entry, root_path, ResolutionOptions::default())
}

/// Lowers an entry module with builtin segment caching and explicit resolution options.
pub fn lower_bundle_cached_with_options(
    entry: &Path,
    root_path: &Path,
    options: ResolutionOptions,
) -> Result<wjsm_ir::Program> {
    lower_bundle_cached_with_debug(entry, root_path, options, false)
}

/// 同 [`lower_bundle_cached_with_options`]，可开启语句级 debug 插桩。
pub fn lower_bundle_cached_with_debug(
    entry: &Path,
    root_path: &Path,
    options: ResolutionOptions,
    emit_debug_checks: bool,
) -> Result<wjsm_ir::Program> {
    let bundler = ModuleBundler::with_resolution_options(root_path, options)?
        .with_emit_debug_checks(emit_debug_checks);
    bundler.lower_bundle_cached(entry)
}

/// 同 [`lower_bundle_cached_with_options`]，但走显式 store。
pub fn lower_bundle_cached_with_store(
    entry: &Path,
    store: ModuleSourceStore,
    options: ResolutionOptions,
) -> Result<wjsm_ir::Program> {
    lower_bundle_cached_with_store_and_debug(entry, store, options, false)
}

/// 同 [`lower_bundle_cached_with_store`]，可开启语句级 debug 插桩。
pub fn lower_bundle_cached_with_store_and_debug(
    entry: &Path,
    store: ModuleSourceStore,
    options: ResolutionOptions,
    emit_debug_checks: bool,
) -> Result<wjsm_ir::Program> {
    ModuleBundler::with_store(store, options)?
        .with_emit_debug_checks(emit_debug_checks)
        .lower_bundle_cached(entry)
}

/// Lowers a runtime-loaded entry module and creates a namespace for that entry.
pub fn lower_runtime_entry_bundle_with_options(
    entry: &Path,
    root_path: &Path,
    options: ResolutionOptions,
) -> Result<RuntimeEntryBundle> {
    lower_runtime_entry_bundle_with_debug(entry, root_path, options, false)
}

/// 同 [`lower_runtime_entry_bundle_with_options`]，可开启 debug 插桩。
pub fn lower_runtime_entry_bundle_with_debug(
    entry: &Path,
    root_path: &Path,
    options: ResolutionOptions,
    emit_debug_checks: bool,
) -> Result<RuntimeEntryBundle> {
    let bundler = ModuleBundler::with_resolution_options(root_path, options)?
        .with_emit_debug_checks(emit_debug_checks);
    bundler.lower_runtime_entry_bundle(entry)
}

/// 同 [`lower_runtime_entry_bundle_with_debug`]，但走显式 store。
pub fn lower_runtime_entry_bundle_with_store(
    entry: &Path,
    store: ModuleSourceStore,
    options: ResolutionOptions,
    emit_debug_checks: bool,
) -> Result<RuntimeEntryBundle> {
    ModuleBundler::with_store(store, options)?
        .with_emit_debug_checks(emit_debug_checks)
        .lower_runtime_entry_bundle(entry)
}

/// 将运行时加载的 Node 内置模块 lower 为 ESM，并为入口创建命名空间对象。
pub fn lower_runtime_builtin_bundle_with_options(
    specifier: &str,
    root_path: &Path,
    options: ResolutionOptions,
) -> Result<RuntimeEntryBundle> {
    lower_runtime_builtin_bundle_with_debug(specifier, root_path, options, false)
}

/// 同 [`lower_runtime_builtin_bundle_with_options`]，可开启 debug 插桩。
pub fn lower_runtime_builtin_bundle_with_debug(
    specifier: &str,
    root_path: &Path,
    options: ResolutionOptions,
    emit_debug_checks: bool,
) -> Result<RuntimeEntryBundle> {
    let bundler = ModuleBundler::with_resolution_options(root_path, options)?
        .with_emit_debug_checks(emit_debug_checks);
    bundler.lower_runtime_builtin_bundle(specifier)
}

/// 同 [`lower_runtime_builtin_bundle_with_debug`]，但走显式 store。
pub fn lower_runtime_builtin_bundle_with_store(
    specifier: &str,
    store: ModuleSourceStore,
    options: ResolutionOptions,
    emit_debug_checks: bool,
) -> Result<RuntimeEntryBundle> {
    ModuleBundler::with_store(store, options)?
        .with_emit_debug_checks(emit_debug_checks)
        .lower_runtime_builtin_bundle(specifier)
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

/// 将入口模块和按 bundle graph 收集到的所有依赖 lower 为 IR `Program`。
///
/// codegen 由调用方负责（本 crate 不依赖具体后端）；失败时错误会携带
/// `entry` 和 `root_path`，方便调用方定位是哪个入口图构建失败。
pub fn bundle_program(entry: &Path, root_path: &Path) -> Result<wjsm_ir::Program> {
    bundle_program_with_options(entry, root_path, ResolutionOptions::default())
}

/// Bundles an entry module into an IR `Program` with explicit package resolution options.
pub fn bundle_program_with_options(
    entry: &Path,
    root_path: &Path,
    options: ResolutionOptions,
) -> Result<wjsm_ir::Program> {
    let bundler = ModuleBundler::with_resolution_options(root_path, options)
        .with_context(|| format!("create module bundler for root {}", root_path.display()))?;
    bundler.bundle_program(entry).with_context(|| {
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
    fn public_bundle_program_function_works() {
        let root = std::env::temp_dir()
            .join("wjsm-test-cache")
            .join("module")
            .join(format!("public-bundle-{}", std::process::id()));
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

        let result = bundle_program(Path::new("main.js"), &root);
        assert!(
            result.is_ok(),
            "public bundle should succeed: {:?}",
            result.err()
        );
        let program = result.unwrap();
        assert!(
            !program.functions().is_empty(),
            "bundled program should contain lowered functions"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn artifact_bytes_do_not_depend_on_absolute_build_root() {
        let temp = std::env::temp_dir().join("wjsm-test-cache").join("module");
        let roots = [
            temp.join(format!("artifact-root-a-{}", std::process::id())),
            temp.join(format!("artifact-root-b-{}", std::process::id())),
        ];
        for root in &roots {
            let _ = std::fs::remove_dir_all(root);
            std::fs::create_dir_all(root).expect("temp project dir should be creatable");
            std::fs::write(root.join("package.json"), r#"{"type":"module"}"#)
                .expect("package should be writable");
            std::fs::write(
                root.join("main.js"),
                "import { value } from './lib.js';\nconsole.log(value);\n",
            )
            .expect("main module should be writable");
            std::fs::write(root.join("lib.js"), "export const value = 42;\n")
                .expect("lib module should be writable");
        }

        let artifacts: Vec<_> = roots
            .iter()
            .map(|root| {
                let input = lower_artifact_input(Path::new("main.js"), root)
                    .expect("artifact input should lower");
                wjsm_artifact_format::PortableArtifact::from_input(&input)
                    .expect("artifact should encode")
            })
            .collect();
        assert_eq!(artifacts[0].bytes(), artifacts[1].bytes());
        assert!(
            artifacts[0]
                .manifest()
                .modules
                .iter()
                .all(|module| !module.logical_url.starts_with('/'))
        );

        for root in &roots {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
