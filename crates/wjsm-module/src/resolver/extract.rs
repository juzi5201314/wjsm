use anyhow::{Result, bail};
use std::collections::HashSet;

use swc_core::ecma::ast;

use super::types::{ExportEntry, ImportEntry};

pub(super) fn extract_imports(module: &ast::Module) -> Vec<ImportEntry> {
    module
        .body
        .iter()
        .filter_map(|item| match item {
            ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import_decl)) => {
                let specifier = import_decl.src.value.to_string_lossy().into_owned();
                let mut names = Vec::new();

                for spec in &import_decl.specifiers {
                    match spec {
                        ast::ImportSpecifier::Named(named) => {
                            let local = named.local.sym.to_string();
                            let imported = named
                                .imported
                                .as_ref()
                                .map(|id| match id {
                                    ast::ModuleExportName::Ident(ident) => ident.sym.to_string(),
                                    ast::ModuleExportName::Str(s) => {
                                        s.value.to_string_lossy().into_owned()
                                    }
                                })
                                .unwrap_or_else(|| local.clone());
                            names.push((local, imported));
                        }
                        ast::ImportSpecifier::Default(default) => {
                            let local = default.local.sym.to_string();
                            names.push((local, "default".to_string()));
                        }
                        ast::ImportSpecifier::Namespace(ns) => {
                            let local = ns.local.sym.to_string();
                            names.push((local, "*".to_string()));
                        }
                    }
                }

                Some(ImportEntry {
                    specifier,
                    names,
                    source_span: import_decl.span,
                })
            }
            _ => None,
        })
        .collect()
}

pub(super) fn extract_exports(module: &ast::Module) -> Vec<ExportEntry> {
    let mut exports = Vec::new();

    for item in &module.body {
        match item {
            ast::ModuleItem::ModuleDecl(decl) => match decl {
                ast::ModuleDecl::ExportNamed(named_export) => {
                    if let Some(src) = &named_export.src {
                        let source = src.value.to_string_lossy().into_owned();
                        for spec in &named_export.specifiers {
                            match spec {
                                ast::ExportSpecifier::Named(named) => {
                                    let local = match &named.orig {
                                        ast::ModuleExportName::Ident(ident) => {
                                            ident.sym.to_string()
                                        }
                                        ast::ModuleExportName::Str(s) => {
                                            s.value.to_string_lossy().into_owned()
                                        }
                                    };
                                    let exported = named
                                        .exported
                                        .as_ref()
                                        .map(|id| match id {
                                            ast::ModuleExportName::Ident(ident) => {
                                                ident.sym.to_string()
                                            }
                                            ast::ModuleExportName::Str(s) => {
                                                s.value.to_string_lossy().into_owned()
                                            }
                                        })
                                        .unwrap_or_else(|| local.clone());
                                    exports.push(ExportEntry::NamedReExport {
                                        local,
                                        exported,
                                        source: source.clone(),
                                    });
                                }
                                ast::ExportSpecifier::Namespace(ns) => {
                                    let name = match &ns.name {
                                        ast::ModuleExportName::Ident(ident) => {
                                            ident.sym.to_string()
                                        }
                                        ast::ModuleExportName::Str(s) => {
                                            s.value.to_string_lossy().into_owned()
                                        }
                                    };
                                    exports.push(ExportEntry::NamedReExport {
                                        local: "*".to_string(),
                                        exported: name,
                                        source: source.clone(),
                                    });
                                }
                                ast::ExportSpecifier::Default(default) => {
                                    let local = default.exported.sym.to_string();
                                    exports.push(ExportEntry::NamedReExport {
                                        local: local.clone(),
                                        exported: "default".to_string(),
                                        source: source.clone(),
                                    });
                                }
                            }
                        }
                    } else {
                        for spec in &named_export.specifiers {
                            match spec {
                                ast::ExportSpecifier::Named(named) => {
                                    let local = match &named.orig {
                                        ast::ModuleExportName::Ident(ident) => {
                                            ident.sym.to_string()
                                        }
                                        ast::ModuleExportName::Str(s) => {
                                            s.value.to_string_lossy().into_owned()
                                        }
                                    };
                                    let exported = named
                                        .exported
                                        .as_ref()
                                        .map(|id| match id {
                                            ast::ModuleExportName::Ident(ident) => {
                                                ident.sym.to_string()
                                            }
                                            ast::ModuleExportName::Str(s) => {
                                                s.value.to_string_lossy().into_owned()
                                            }
                                        })
                                        .unwrap_or_else(|| local.clone());
                                    exports.push(ExportEntry::Named { local, exported });
                                }
                                ast::ExportSpecifier::Default(default) => {
                                    let local = default.exported.sym.to_string();
                                    exports.push(ExportEntry::Named {
                                        local,
                                        exported: "default".to_string(),
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                }
                ast::ModuleDecl::ExportDefaultExpr(_default_expr) => {
                    exports.push(ExportEntry::Default {
                        local: "_default_export".to_string(),
                    });
                }
                ast::ModuleDecl::ExportDefaultDecl(default_decl) => {
                    let local = match &default_decl.decl {
                        ast::DefaultDecl::Class(class) => class
                            .ident
                            .as_ref()
                            .map(|i| i.sym.to_string())
                            .unwrap_or_else(|| "_default_export".to_string()),
                        ast::DefaultDecl::Fn(func) => func
                            .ident
                            .as_ref()
                            .map(|i| i.sym.to_string())
                            .unwrap_or_else(|| "_default_export".to_string()),
                        ast::DefaultDecl::TsInterfaceDecl(_) => "_default_export".to_string(),
                    };
                    exports.push(ExportEntry::Default { local });
                }
                ast::ModuleDecl::ExportAll(all) => {
                    exports.push(ExportEntry::All {
                        source: all.src.value.to_string_lossy().into_owned(),
                    });
                }
                ast::ModuleDecl::ExportDecl(export_decl) => {
                    let name = match &export_decl.decl {
                        ast::Decl::Class(class) => class.ident.sym.to_string(),
                        ast::Decl::Fn(func) => func.ident.sym.to_string(),
                        ast::Decl::Var(var) => {
                            for decl in &var.decls {
                                if let ast::Pat::Ident(ident) = &decl.name {
                                    exports.push(ExportEntry::Declaration {
                                        name: ident.id.sym.to_string(),
                                    });
                                }
                            }
                            continue;
                        }
                        ast::Decl::TsInterface(_) => continue,
                        ast::Decl::TsTypeAlias(_) => continue,
                        ast::Decl::TsEnum(_) => continue,
                        ast::Decl::TsModule(_) => continue,
                        ast::Decl::Using(_) => continue,
                    };
                    exports.push(ExportEntry::Declaration { name });
                }
                _ => {}
            },
            _ => {}
        }
    }

    exports
}

pub fn extract_dynamic_imports(module: &ast::Module) -> Result<Vec<String>> {
    let mut specifiers = Vec::new();
    for item in &module.body {
        extract_dynamic_imports_from_item(item, &mut specifiers)?;
    }
    Ok(specifiers)
}

fn extract_dynamic_imports_from_item(
    item: &ast::ModuleItem,
    specifiers: &mut Vec<String>,
) -> Result<()> {
    match item {
        ast::ModuleItem::ModuleDecl(decl) => {
            extract_dynamic_imports_from_module_decl(decl, specifiers)?;
        }
        ast::ModuleItem::Stmt(stmt) => {
            extract_dynamic_imports_from_stmt(stmt, specifiers)?;
        }
    }
    Ok(())
}

fn extract_dynamic_imports_from_module_decl(
    decl: &ast::ModuleDecl,
    specifiers: &mut Vec<String>,
) -> Result<()> {
    match decl {
        ast::ModuleDecl::ExportDecl(export_decl) => {
            extract_dynamic_imports_from_decl(&export_decl.decl, specifiers)?;
        }
        ast::ModuleDecl::ExportDefaultExpr(default_expr) => {
            extract_dynamic_imports_from_expr(&default_expr.expr, specifiers)?;
        }
        _ => {}
    }
    Ok(())
}

fn extract_dynamic_imports_from_decl(
    decl: &ast::Decl,
    specifiers: &mut Vec<String>,
) -> Result<()> {
    match decl {
        ast::Decl::Fn(fn_decl) => {
            extract_dynamic_imports_from_function(&fn_decl.function, specifiers)?;
        }
        ast::Decl::Class(class_decl) => {
            extract_dynamic_imports_from_class(&class_decl.class, specifiers)?;
        }
        ast::Decl::Var(var_decl) => {
            for declarator in &var_decl.decls {
                if let Some(init) = &declarator.init {
                    extract_dynamic_imports_from_expr(init, specifiers)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn extract_dynamic_imports_from_stmt(
    stmt: &ast::Stmt,
    specifiers: &mut Vec<String>,
) -> Result<()> {
    match stmt {
        ast::Stmt::Expr(expr_stmt) => {
            extract_dynamic_imports_from_expr(&expr_stmt.expr, specifiers)?;
        }
        ast::Stmt::Decl(decl) => {
            extract_dynamic_imports_from_decl(decl, specifiers)?;
        }
        ast::Stmt::Block(block) => {
            for s in &block.stmts {
                extract_dynamic_imports_from_stmt(s, specifiers)?;
            }
        }
        ast::Stmt::If(if_stmt) => {
            extract_dynamic_imports_from_expr(&if_stmt.test, specifiers)?;
            extract_dynamic_imports_from_stmt(&if_stmt.cons, specifiers)?;
            if let Some(alt) = &if_stmt.alt {
                extract_dynamic_imports_from_stmt(alt, specifiers)?;
            }
        }
        ast::Stmt::For(for_stmt) => {
            if let Some(init) = &for_stmt.init {
                match init {
                    ast::VarDeclOrExpr::VarDecl(var_decl) => {
                        extract_dynamic_imports_from_decl(
                            &ast::Decl::Var(var_decl.clone()),
                            specifiers,
                        )?;
                    }
                    ast::VarDeclOrExpr::Expr(expr) => {
                        extract_dynamic_imports_from_expr(expr, specifiers)?;
                    }
                }
            }
            if let Some(test) = &for_stmt.test {
                extract_dynamic_imports_from_expr(test, specifiers)?;
            }
            if let Some(update) = &for_stmt.update {
                extract_dynamic_imports_from_expr(update, specifiers)?;
            }
            extract_dynamic_imports_from_stmt(&for_stmt.body, specifiers)?;
        }
        ast::Stmt::ForIn(for_in) => {
            extract_dynamic_imports_from_stmt(&for_in.body, specifiers)?;
        }
        ast::Stmt::ForOf(for_of) => {
            extract_dynamic_imports_from_stmt(&for_of.body, specifiers)?;
        }
        ast::Stmt::While(while_stmt) => {
            extract_dynamic_imports_from_expr(&while_stmt.test, specifiers)?;
            extract_dynamic_imports_from_stmt(&while_stmt.body, specifiers)?;
        }
        ast::Stmt::DoWhile(do_while) => {
            extract_dynamic_imports_from_stmt(&do_while.body, specifiers)?;
            extract_dynamic_imports_from_expr(&do_while.test, specifiers)?;
        }
        ast::Stmt::Switch(switch) => {
            extract_dynamic_imports_from_expr(&switch.discriminant, specifiers)?;
            for case in &switch.cases {
                for s in &case.cons {
                    extract_dynamic_imports_from_stmt(s, specifiers)?;
                }
            }
        }
        ast::Stmt::Try(try_stmt) => {
            extract_dynamic_imports_from_stmt(
                &ast::Stmt::Block(try_stmt.block.clone()),
                specifiers,
            )?;
            if let Some(handler) = &try_stmt.handler {
                extract_dynamic_imports_from_stmt(
                    &ast::Stmt::Block(handler.body.clone()),
                    specifiers,
                )?;
            }
            if let Some(finalizer) = &try_stmt.finalizer {
                extract_dynamic_imports_from_stmt(
                    &ast::Stmt::Block(finalizer.clone()),
                    specifiers,
                )?;
            }
        }
        ast::Stmt::Labeled(labeled) => {
            extract_dynamic_imports_from_stmt(&labeled.body, specifiers)?;
        }
        _ => {}
    }
    Ok(())
}

fn extract_dynamic_imports_from_expr(
    expr: &ast::Expr,
    specifiers: &mut Vec<String>,
) -> Result<()> {
    match expr {
        ast::Expr::Call(call) => {
            if matches!(call.callee, ast::Callee::Import(_)) {
                let specifier = extract_import_call_specifier(call)?;
                specifiers.push(specifier);
            } else {
                if let ast::Callee::Expr(callee_expr) = &call.callee {
                    extract_dynamic_imports_from_expr(callee_expr, specifiers)?;
                }
                for arg in &call.args {
                    extract_dynamic_imports_from_expr(&arg.expr, specifiers)?;
                }
            }
        }
        ast::Expr::Bin(bin) => {
            extract_dynamic_imports_from_expr(&bin.left, specifiers)?;
            extract_dynamic_imports_from_expr(&bin.right, specifiers)?;
        }
        ast::Expr::Unary(unary) => {
            extract_dynamic_imports_from_expr(&unary.arg, specifiers)?;
        }
        ast::Expr::Assign(assign) => {
            extract_dynamic_imports_from_expr(assign.right.as_ref(), specifiers)?;
        }
        ast::Expr::Cond(cond) => {
            extract_dynamic_imports_from_expr(&cond.test, specifiers)?;
            extract_dynamic_imports_from_expr(&cond.cons, specifiers)?;
            extract_dynamic_imports_from_expr(&cond.alt, specifiers)?;
        }
        ast::Expr::Member(member) => {
            extract_dynamic_imports_from_expr(&member.obj, specifiers)?;
            if let ast::MemberProp::Computed(computed) = &member.prop {
                extract_dynamic_imports_from_expr(&computed.expr, specifiers)?;
            }
        }
        ast::Expr::Object(obj) => {
            for prop in &obj.props {
                match prop {
                    ast::PropOrSpread::Prop(p) => match p.as_ref() {
                        ast::Prop::KeyValue(kv) => {
                            extract_dynamic_imports_from_expr(&kv.value, specifiers)?;
                        }
                        ast::Prop::Shorthand(_) => {}
                        ast::Prop::Getter(getter) => {
                            if let Some(body) = &getter.body {
                                for s in &body.stmts {
                                    extract_dynamic_imports_from_stmt(s, specifiers)?;
                                }
                            }
                        }
                        ast::Prop::Setter(setter) => {
                            if let Some(body) = &setter.body {
                                for s in &body.stmts {
                                    extract_dynamic_imports_from_stmt(s, specifiers)?;
                                }
                            }
                        }
                        ast::Prop::Method(method) => {
                            extract_dynamic_imports_from_function(
                                &method.function, specifiers,
                            )?;
                        }
                        _ => {}
                    },
                    ast::PropOrSpread::Spread(spread) => {
                        extract_dynamic_imports_from_expr(&spread.expr, specifiers)?;
                    }
                }
            }
        }
        ast::Expr::Array(arr) => {
            for elem in &arr.elems {
                if let Some(elem) = elem {
                    extract_dynamic_imports_from_expr(&elem.expr, specifiers)?;
                }
            }
        }
        ast::Expr::Arrow(arrow) => {
            match &*arrow.body {
                ast::BlockStmtOrExpr::BlockStmt(block) => {
                    for s in &block.stmts {
                        extract_dynamic_imports_from_stmt(s, specifiers)?;
                    }
                }
                ast::BlockStmtOrExpr::Expr(expr) => {
                    extract_dynamic_imports_from_expr(expr, specifiers)?;
                }
            }
        }
        ast::Expr::Fn(fn_expr) => {
            extract_dynamic_imports_from_function(&fn_expr.function, specifiers)?;
        }
        ast::Expr::Class(class_expr) => {
            extract_dynamic_imports_from_class(&class_expr.class, specifiers)?;
        }
        ast::Expr::Tpl(tpl) => {
            for expr in &tpl.exprs {
                extract_dynamic_imports_from_expr(expr, specifiers)?;
            }
        }
        ast::Expr::TaggedTpl(tagged) => {
            extract_dynamic_imports_from_expr(&tagged.tag, specifiers)?;
            for expr in &tagged.tpl.exprs {
                extract_dynamic_imports_from_expr(expr, specifiers)?;
            }
        }
        ast::Expr::Paren(paren) => {
            extract_dynamic_imports_from_expr(&paren.expr, specifiers)?;
        }
        ast::Expr::Seq(seq) => {
            for expr in &seq.exprs {
                extract_dynamic_imports_from_expr(expr, specifiers)?;
            }
        }
        ast::Expr::New(new) => {
            extract_dynamic_imports_from_expr(&new.callee, specifiers)?;
            if let Some(args) = &new.args {
                for arg in args {
                    extract_dynamic_imports_from_expr(&arg.expr, specifiers)?;
                }
            }
        }
        ast::Expr::Await(await_expr) => {
            extract_dynamic_imports_from_expr(&await_expr.arg, specifiers)?;
        }
        ast::Expr::Yield(yield_expr) => {
            if let Some(arg) = &yield_expr.arg {
                extract_dynamic_imports_from_expr(arg, specifiers)?;
            }
        }
        ast::Expr::MetaProp(_) | ast::Expr::Ident(_) | ast::Expr::Lit(_) => {}
        _ => {}
    }
    Ok(())
}

fn extract_dynamic_imports_from_function(
    function: &ast::Function,
    specifiers: &mut Vec<String>,
) -> Result<()> {
    if let Some(body) = &function.body {
        for s in &body.stmts {
            extract_dynamic_imports_from_stmt(s, specifiers)?;
        }
    }
    Ok(())
}

fn extract_dynamic_imports_from_class(
    class: &ast::Class,
    specifiers: &mut Vec<String>,
) -> Result<()> {
    for member in &class.body {
        match member {
            ast::ClassMember::Method(method) => {
                extract_dynamic_imports_from_function(&method.function, specifiers)?;
            }
            ast::ClassMember::Constructor(ctor) => {
                if let Some(body) = &ctor.body {
                    for s in &body.stmts {
                        extract_dynamic_imports_from_stmt(s, specifiers)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn extract_import_call_specifier(call: &ast::CallExpr) -> Result<String> {
    let first_arg = call.args.first().ok_or_else(|| {
        anyhow::anyhow!(
            "import() requires a module specifier; \
             in AOT compilation mode, only string literal specifiers are supported"
        )
    })?;

    match first_arg.expr.as_ref() {
        ast::Expr::Lit(ast::Lit::Str(s)) => Ok(s.value.to_string_lossy().into_owned()),
        ast::Expr::Tpl(tpl) => {
            if tpl.exprs.is_empty() {
                let mut result = String::new();
                for quasi in &tpl.quasis {
                    result.push_str(&quasi.raw);
                }
                Ok(result)
            } else {
                bail!(
                    "import() with template literal containing expressions is not supported; \
                     AOT compilation requires the specifier to be a static string literal"
                )
            }
        }
        _ => {
            bail!(
                "import() requires a string literal specifier; \
                 AOT compilation cannot resolve dynamic specifiers at compile time. \
                 Use a string literal like import('./module.js') instead"
            )
        }
    }
}

pub(super) fn add_synthetic_default_export(
    ast: &mut swc_core::ecma::ast::Module,
    exports: &[ExportEntry],
) {
    use swc_core::common::{DUMMY_SP, SyntaxContext};

    let mut export_names: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for entry in exports {
        match entry {
            ExportEntry::Named { exported, .. } => {
                if exported != "default" && seen.insert(exported.clone()) {
                    export_names.push(exported.clone());
                }
            }
            ExportEntry::Declaration { name } => {
                if seen.insert(name.clone()) {
                    export_names.push(name.clone());
                }
            }
            _ => {}
        }
    }

    if export_names.is_empty() {
        return;
    }

    let props: Vec<swc_core::ecma::ast::PropOrSpread> = export_names
        .iter()
        .map(|name| {
            swc_core::ecma::ast::PropOrSpread::Prop(Box::new(swc_core::ecma::ast::Prop::Shorthand(
                swc_core::ecma::ast::Ident::new(name.clone().into(), DUMMY_SP, SyntaxContext::default()),
            )))
        })
        .collect();

    let default_export = swc_core::ecma::ast::ModuleItem::ModuleDecl(
        swc_core::ecma::ast::ModuleDecl::ExportDefaultExpr(swc_core::ecma::ast::ExportDefaultExpr {
            span: DUMMY_SP,
            expr: Box::new(swc_core::ecma::ast::Expr::Object(swc_core::ecma::ast::ObjectLit {
                span: DUMMY_SP,
                props,
            })),
        }),
    );

    ast.body.push(default_export);
}
