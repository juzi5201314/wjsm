use swc_core::common::{DUMMY_SP, SyntaxContext};
use swc_core::ecma::ast;

pub(super) fn extract_require_specifier(call: &ast::CallExpr) -> Option<String> {
    if let ast::Callee::Expr(expr) = &call.callee {
        if let ast::Expr::Ident(ident) = expr.as_ref() {
            if ident.sym.as_ref() == "require" && call.args.len() == 1 {
                if let ast::Expr::Lit(ast::Lit::Str(s)) = call.args[0].expr.as_ref() {
                    return Some(s.value.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

pub(super) fn is_module_exports_member(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Member(member) => {
            if let ast::Expr::Ident(module_ident) = member.obj.as_ref() {
                if module_ident.sym.as_ref() == "module" {
                    if let ast::MemberProp::Ident(exports_ident) = &member.prop {
                        if exports_ident.sym.as_ref() == "exports" {
                            return true;
                        }
                    }
                }
            }
            false
        }
        _ => false,
    }
}

pub(super) fn is_module_exports_member_no_prop(obj: &ast::Expr, prop: &ast::MemberProp) -> bool {
    if let ast::Expr::Ident(module_ident) = obj {
        if module_ident.sym.as_ref() == "module" {
            if let ast::MemberProp::Ident(exports_ident) = prop {
                if exports_ident.sym.as_ref() == "exports" {
                    return true;
                }
            }
        }
    }
    false
}

pub(super) fn is_exports_member(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Member(member) => {
            if let ast::Expr::Ident(exports_ident) = member.obj.as_ref() {
                if exports_ident.sym.as_ref() == "exports" {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

pub(super) fn is_exports_ident(expr: &ast::Expr) -> bool {
    matches!(expr, ast::Expr::Ident(ident) if ident.sym.as_ref() == "exports")
}

pub(super) fn create_import_default_decl(specifier: &str, local_name: &str) -> ast::ImportDecl {
    ast::ImportDecl {
        span: DUMMY_SP,
        phase: ast::ImportPhase::Evaluation,
        specifiers: vec![ast::ImportSpecifier::Default(ast::ImportDefaultSpecifier {
            span: DUMMY_SP,
            local: ast::Ident::new(local_name.into(), DUMMY_SP, SyntaxContext::default()),
        })],
        src: Box::new(ast::Str {
            span: DUMMY_SP,
            value: specifier.into(),
            raw: None,
        }),
        type_only: false,
        with: None,
    }
}

pub(super) fn create_synthetic_default_export(export_names: &[(String, String)]) -> ast::Expr {
    let props: Vec<ast::PropOrSpread> = export_names
        .iter()
        .map(|(prop_name, var_name)| {
            ast::PropOrSpread::Prop(Box::new(ast::Prop::KeyValue(ast::KeyValueProp {
                key: ast::PropName::Ident(ast::IdentName::new(prop_name.clone().into(), DUMMY_SP)),
                value: Box::new(ast::Expr::Ident(ast::Ident::new(
                    var_name.clone().into(),
                    DUMMY_SP,
                    SyntaxContext::default(),
                ))),
            })))
        })
        .collect();
    ast::Expr::Object(ast::ObjectLit {
        span: DUMMY_SP,
        props,
    })
}

pub(super) fn create_let_decl(name: &str, value: ast::Expr) -> ast::Decl {
    ast::Decl::Var(Box::new(ast::VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::default(),
        kind: ast::VarDeclKind::Let,
        declare: false,
        decls: vec![ast::VarDeclarator {
            span: DUMMY_SP,
            name: ast::Pat::Ident(ast::BindingIdent {
                id: ast::Ident::new(name.into(), DUMMY_SP, SyntaxContext::default()),
                type_ann: None,
            }),
            init: Some(Box::new(value)),
            definite: false,
        }],
    }))
}
