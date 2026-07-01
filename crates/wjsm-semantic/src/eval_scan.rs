use super::*;

pub(crate) fn direct_eval_predeclare_code(
    expr: &swc_ast::Expr,
    eval_string_bindings: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let swc_ast::Expr::Call(call) = expr else {
        return None;
    };
    let swc_ast::Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let swc_ast::Expr::Ident(ident) = callee.as_ref() else {
        return None;
    };
    if ident.sym.as_ref() != "eval" {
        return None;
    }
    let first = call.args.first()?;
    literal_string_expr(&Some(first.expr.clone())).or_else(|| {
        let swc_ast::Expr::Ident(arg_ident) = first.expr.as_ref() else {
            return None;
        };
        eval_string_bindings.get(arg_ident.sym.as_ref()).cloned()
    })
}

pub(crate) fn literal_string_expr(expr: &Option<Box<swc_ast::Expr>>) -> Option<String> {
    let swc_ast::Expr::Lit(swc_ast::Lit::Str(string)) = expr.as_deref()? else {
        return None;
    };
    Some(string.value.to_string_lossy().into_owned())
}

pub(crate) fn eval_code_has_use_strict_directive(code: &str) -> bool {
    let bytes = code.as_bytes();
    let mut index = 0;
    skip_js_trivia(bytes, &mut index);

    while let Some(quote @ (b'\'' | b'"')) = bytes.get(index).copied() {
        index += 1;
        let literal_start = index;
        while index < bytes.len() && bytes[index] != quote {
            if bytes[index] == b'\\' {
                return false;
            }
            index += 1;
        }
        if index >= bytes.len() {
            return false;
        }

        let directive = &code[literal_start..index];
        index += 1;
        skip_js_trivia(bytes, &mut index);
        if bytes.get(index) == Some(&b';') {
            index += 1;
        }

        if directive == "use strict" {
            return true;
        }

        skip_js_trivia(bytes, &mut index);
    }

    false
}

pub(crate) fn skip_js_trivia(bytes: &[u8], index: &mut usize) {
    loop {
        while *index < bytes.len() && bytes[*index].is_ascii_whitespace() {
            *index += 1;
        }

        if bytes.get(*index..*index + 2) == Some(b"//") {
            *index += 2;
            while *index < bytes.len() && !matches!(bytes[*index], b'\n' | b'\r') {
                *index += 1;
            }
            continue;
        }

        if bytes.get(*index..*index + 2) == Some(b"/*") {
            *index += 2;
            while *index + 1 < bytes.len() && bytes.get(*index..*index + 2) != Some(b"*/") {
                *index += 1;
            }
            if *index + 1 < bytes.len() {
                *index += 2;
            }
            continue;
        }

        break;
    }
}

pub(crate) fn module_has_use_strict_directive(module: &swc_ast::Module) -> bool {
    let mut found = false;
    for item in &module.body {
        let swc_ast::ModuleItem::Stmt(swc_ast::Stmt::Expr(expr_stmt)) = item else {
            break;
        };
        let swc_ast::Expr::Lit(swc_ast::Lit::Str(string)) = expr_stmt.expr.as_ref() else {
            break;
        };
        if string.value.as_str() == Some("use strict") {
            found = true;
        }
    }
    found
}

pub fn eval_literal_binding_names(code: &str) -> Vec<String> {
    let Ok(module) = wjsm_parser::parse_script_as_module(code) else {
        return Vec::new();
    };

    let mut names = Vec::new();
    collect_eval_var_declared_names_from_module(&module, &mut names);
    names
}

fn push_unique_name(names: &mut Vec<String>, name: String) {
    if !names.iter().any(|existing| existing == &name) {
        names.push(name);
    }
}

fn collect_eval_var_declared_names_from_module(module: &swc_ast::Module, names: &mut Vec<String>) {
    for item in &module.body {
        if let swc_ast::ModuleItem::Stmt(stmt) = item {
            collect_eval_var_declared_names_from_stmt(stmt, names);
        }
    }
}

fn collect_eval_var_declared_names_from_stmts(stmts: &[swc_ast::Stmt], names: &mut Vec<String>) {
    for stmt in stmts {
        collect_eval_var_declared_names_from_stmt(stmt, names);
    }
}

fn collect_eval_var_declared_names_from_stmt(stmt: &swc_ast::Stmt, names: &mut Vec<String>) {
    match stmt {
        swc_ast::Stmt::Decl(swc_ast::Decl::Var(var_decl)) => {
            collect_eval_var_declared_names_from_var_decl(var_decl, names);
        }
        swc_ast::Stmt::Decl(swc_ast::Decl::Fn(fn_decl)) => {
            push_unique_name(names, fn_decl.ident.sym.to_string());
        }
        swc_ast::Stmt::Block(block) => {
            collect_eval_var_declared_names_from_stmts(&block.stmts, names);
        }
        swc_ast::Stmt::If(if_stmt) => {
            collect_eval_var_declared_names_from_stmt(&if_stmt.cons, names);
            if let Some(alt) = &if_stmt.alt {
                collect_eval_var_declared_names_from_stmt(alt, names);
            }
        }
        swc_ast::Stmt::Switch(switch_stmt) => {
            for case in &switch_stmt.cases {
                collect_eval_var_declared_names_from_stmts(&case.cons, names);
            }
        }
        swc_ast::Stmt::Try(try_stmt) => {
            collect_eval_var_declared_names_from_stmts(&try_stmt.block.stmts, names);
            if let Some(handler) = &try_stmt.handler {
                collect_eval_var_declared_names_from_stmts(&handler.body.stmts, names);
            }
            if let Some(finalizer) = &try_stmt.finalizer {
                collect_eval_var_declared_names_from_stmts(&finalizer.stmts, names);
            }
        }
        swc_ast::Stmt::For(for_stmt) => {
            if let Some(swc_ast::VarDeclOrExpr::VarDecl(var_decl)) = &for_stmt.init {
                collect_eval_var_declared_names_from_var_decl(var_decl, names);
            }
            collect_eval_var_declared_names_from_stmt(&for_stmt.body, names);
        }
        swc_ast::Stmt::ForIn(for_in) => {
            if let swc_ast::ForHead::VarDecl(var_decl) = &for_in.left {
                collect_eval_var_declared_names_from_var_decl(var_decl, names);
            }
            collect_eval_var_declared_names_from_stmt(&for_in.body, names);
        }
        swc_ast::Stmt::ForOf(for_of) => {
            if let swc_ast::ForHead::VarDecl(var_decl) = &for_of.left {
                collect_eval_var_declared_names_from_var_decl(var_decl, names);
            }
            collect_eval_var_declared_names_from_stmt(&for_of.body, names);
        }
        swc_ast::Stmt::While(while_stmt) => {
            collect_eval_var_declared_names_from_stmt(&while_stmt.body, names);
        }
        swc_ast::Stmt::DoWhile(do_while) => {
            collect_eval_var_declared_names_from_stmt(&do_while.body, names);
        }
        swc_ast::Stmt::Labeled(labeled) => {
            collect_eval_var_declared_names_from_stmt(&labeled.body, names);
        }
        _ => {}
    }
}

fn collect_eval_var_declared_names_from_var_decl(
    var_decl: &swc_ast::VarDecl,
    names: &mut Vec<String>,
) {
    if !matches!(var_decl.kind, swc_ast::VarDeclKind::Var) {
        return;
    }
    for declarator in &var_decl.decls {
        let mut declarator_names = Vec::new();
        Lowerer::extract_pat_bindings(
            std::slice::from_ref(&declarator.name),
            &mut declarator_names,
        );
        for name in declarator_names {
            push_unique_name(names, name);
        }
    }
}
