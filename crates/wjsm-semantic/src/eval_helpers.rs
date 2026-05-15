use swc_core::ecma::ast as swc_ast;

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
    for item in &module.body {
        let swc_ast::ModuleItem::Stmt(swc_ast::Stmt::Expr(expr_stmt)) = item else {
            return false;
        };
        let swc_ast::Expr::Lit(swc_ast::Lit::Str(string)) = expr_stmt.expr.as_ref() else {
            return false;
        };
        if string.value.as_str() == Some("use strict") {
            return true;
        }
    }
    false
}

pub(crate) fn eval_literal_binding_names(code: &str) -> Vec<String> {
    let mut names = Vec::new();
    let bytes = code.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if is_word_at(bytes, index, b"var") {
            index += 3;
            loop {
                while index < bytes.len()
                    && (bytes[index].is_ascii_whitespace() || bytes[index] == b',')
                {
                    index += 1;
                }
                if index >= bytes.len() || !is_ident_start(bytes[index]) {
                    break;
                }
                let start = index;
                index += 1;
                while index < bytes.len() && is_ident_continue(bytes[index]) {
                    index += 1;
                }
                let name = &code[start..index];
                if !names.iter().any(|existing| existing == name) {
                    names.push(name.to_string());
                }
                while index < bytes.len()
                    && bytes[index] != b','
                    && bytes[index] != b';'
                    && bytes[index] != b'\n'
                {
                    index += 1;
                }
                if index >= bytes.len() || bytes[index] != b',' {
                    break;
                }
            }
        } else if is_word_at(bytes, index, b"function") {
            index += "function".len();
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if index < bytes.len() && bytes[index] == b'*' {
                index += 1;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
            }
            if index < bytes.len() && is_ident_start(bytes[index]) {
                let start = index;
                index += 1;
                while index < bytes.len() && is_ident_continue(bytes[index]) {
                    index += 1;
                }
                let name = &code[start..index];
                if !names.iter().any(|existing| existing == name) {
                    names.push(name.to_string());
                }
            }
        }
        index += 1;
    }
    names
}

pub(crate) fn is_word_at(bytes: &[u8], index: usize, word: &[u8]) -> bool {
    bytes.get(index..index + word.len()) == Some(word)
        && index
            .checked_sub(1)
            .map_or(true, |prev| !is_ident_continue(bytes[prev]))
        && bytes
            .get(index + word.len())
            .map_or(true, |next| !is_ident_continue(*next))
}

pub(crate) fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte == b'$' || byte.is_ascii_alphabetic()
}

pub(crate) fn is_ident_continue(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}
