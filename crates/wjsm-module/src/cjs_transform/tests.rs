use super::*;
use super::helpers::*;
use swc_core::common::{DUMMY_SP, SyntaxContext};
use wjsm_parser;

fn parse(source: &str) -> ast::Module {
    wjsm_parser::parse_module(source).expect("parse should succeed")
}

fn has_import_with_local(transformed: &ast::Module, local: &str) -> bool {
    transformed.body.iter().any(|item| {
        if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) = item {
            import.specifiers.iter().any(|s| {
                if let ast::ImportSpecifier::Default(d) = s {
                    d.local.sym.as_ref() == local
                } else {
                    false
                }
            })
        } else {
            false
        }
    })
}

fn has_let_decl(transformed: &ast::Module) -> bool {
    transformed.body.iter().any(|item| {
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
            var.kind == ast::VarDeclKind::Let
        } else {
            false
        }
    })
}

fn has_default_export(transformed: &ast::Module) -> bool {
    transformed.body.iter().any(|item| {
        matches!(
            item,
            ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultExpr(_))
        )
    })
}

fn has_import_decl(transformed: &ast::Module) -> bool {
    transformed.body.iter().any(|item| {
        matches!(
            item,
            ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(_))
        )
    })
}

#[test]
fn detects_commonjs_require() {
    let module = parse(r#"const foo = require('./foo');"#);
    assert!(is_commonjs_module(&module));
}

#[test]
fn detects_commonjs_module_exports() {
    let module = parse(r#"module.exports.foo = 1;"#);
    assert!(is_commonjs_module(&module));
}

#[test]
fn detects_commonjs_exports() {
    let module = parse(r#"exports.bar = 2;"#);
    assert!(is_commonjs_module(&module));
}

#[test]
fn does_not_detect_plain_module() {
    let module = parse(r#"const x = 1; console.log(x);"#);
    assert!(!is_commonjs_module(&module));
}

#[test]
fn detects_cjs_via_assign_to_exports_ident() {
    let module = parse(r#"exports.foo = 1;"#);
    assert!(is_commonjs_module(&module));
}

#[test]
fn does_not_detect_cjs_for_member_access_only() {
    let module = parse(r#"const x = 1; console.log(x);"#);
    assert!(!is_commonjs_module(&module));
}

#[test]
fn transforms_require() {
    let module = parse(r#"const foo = require('./foo'); console.log(foo);"#);
    let transformed = transform(&module);
    assert!(
        has_import_decl(&transformed),
        "transformed module should have default import decl"
    );
}

#[test]
fn transforms_module_exports() {
    let module = parse(r#"module.exports.foo = 42;"#);
    let transformed = transform(&module);
    assert!(
        has_let_decl(&transformed),
        "transformed module should have let decl"
    );
    assert!(
        has_default_export(&transformed),
        "transformed module should have synthetic default export"
    );
}

#[test]
fn transforms_exports_alias() {
    let module = parse(r#"exports.bar = 42;"#);
    let transformed = transform(&module);
    assert!(
        has_let_decl(&transformed),
        "transformed module should have let decl"
    );
    assert!(
        has_default_export(&transformed),
        "transformed module should have synthetic default export"
    );
}

#[test]
fn transforms_module_exports_default() {
    let module = parse(r#"module.exports = { foo: 1 };"#);
    let transformed = transform(&module);
    assert!(
        has_default_export(&transformed),
        "transformed module should have default export"
    );
}

#[test]
fn require_direct_import_uses_user_var_name() {
    let module = parse(r#"const lib = require('./lib'); console.log(lib);"#);
    let transformed = transform(&module);
    assert!(
        has_import_with_local(&transformed, "lib"),
        "import should use user variable name 'lib'"
    );
    let has_const_lib = transformed.body.iter().any(|item| {
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
            var.decls.iter().any(|d| {
                if let ast::Pat::Ident(b) = &d.name {
                    b.id.sym.as_ref() == "lib"
                } else {
                    false
                }
            })
        } else {
            false
        }
    });
    assert!(
        !has_const_lib,
        "should not have const lib = ... declaration"
    );
}

#[test]
fn transform_preserves_module_decl_items() {
    let module = parse(r#"import { x } from './esm.js'; module.exports.foo = 1;"#);
    let transformed = transform(&module);
    let has_esm_import = transformed.body.iter().any(|item| {
        if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) = item {
            import.specifiers.iter().any(|s| {
                if let ast::ImportSpecifier::Named(n) = s {
                    n.local.sym.as_ref() == "x"
                } else {
                    false
                }
            })
        } else {
            false
        }
    });
    assert!(has_esm_import, "ESM import should be preserved");
}

#[test]
fn transform_with_prefix_adds_prefix_to_var_names() {
    let module = parse(r#"module.exports.foo = 42;"#);
    let transformed = transform_with_prefix(&module, "_1_");
    let has_prefixed_var = transformed.body.iter().any(|item| {
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
            var.decls.iter().any(|d| {
                if let ast::Pat::Ident(b) = &d.name {
                    b.id.sym.as_ref() == "_1___cjs_foo"
                } else {
                    false
                }
            })
        } else {
            false
        }
    });
    assert!(
        has_prefixed_var,
        "should have prefixed variable name _1___cjs_foo"
    );
}

#[test]
fn transform_skips_synthetic_default_when_has_default() {
    let module = parse(r#"module.exports = { foo: 1 }; module.exports.bar = 2;"#);
    let transformed = transform(&module);
    let default_export_count = transformed
        .body
        .iter()
        .filter(|item| {
            matches!(
                item,
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultExpr(_))
            )
        })
        .count();
    assert_eq!(
        default_export_count, 1,
        "should have exactly one default export"
    );
}

#[test]
fn multiple_require_same_specifier_uses_first() {
    let module =
        parse(r#"const a = require('./foo'); const b = require('./foo'); console.log(a, b);"#);
    let transformed = transform(&module);
    let import_count = transformed
        .body
        .iter()
        .filter(|item| {
            matches!(
                item,
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(_))
            )
        })
        .count();
    assert_eq!(
        import_count, 1,
        "same specifier should produce only one import"
    );
}

#[test]
fn require_in_non_var_context_generates_auto_name() {
    let module = parse(r#"console.log(require('./foo'));"#);
    let transformed = transform(&module);
    let has_auto_import = transformed.body.iter().any(|item| {
        if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) = item {
            import.specifiers.iter().any(|s| {
                if let ast::ImportSpecifier::Default(d) = s {
                    d.local.sym.as_ref().starts_with("__cjs_req_")
                } else {
                    false
                }
            })
        } else {
            false
        }
    });
    assert!(
        has_auto_import,
        "non-var require should generate __cjs_req_N import"
    );
}

#[test]
fn non_assign_expr_stmt_not_transformed() {
    let module = parse(r#"console.log(1);"#);
    let transformed = transform(&module);
    let has_expr_stmt = transformed
        .body
        .iter()
        .any(|item| matches!(item, ast::ModuleItem::Stmt(ast::Stmt::Expr(_))));
    assert!(
        has_expr_stmt,
        "non-assign expression statement should be preserved"
    );
}

#[test]
fn compound_assign_not_transformed() {
    let module = parse(r#"let x = 1; x += 2;"#);
    let transformed = transform(&module);
    assert!(
        !has_default_export(&transformed),
        "compound assignment should not produce default export"
    );
}

#[test]
fn non_member_assign_not_transformed() {
    let module = parse(r#"let x = 1; x = 2;"#);
    let transformed = transform(&module);
    assert!(
        !has_default_export(&transformed),
        "simple assignment should not produce default export"
    );
}

#[test]
fn computed_string_property_exports() {
    let module = parse(r#"exports['foo'] = 42;"#);
    let transformed = transform(&module);
    assert!(
        has_let_decl(&transformed),
        "computed string property should produce let decl"
    );
    assert!(
        has_default_export(&transformed),
        "computed string property should produce synthetic default export"
    );
}

#[test]
fn computed_non_string_property_not_transformed() {
    let module = parse(r#"let key = 'foo'; exports[key] = 42;"#);
    let transformed = transform(&module);
    let cjs_let_count = transformed
        .body
        .iter()
        .filter(|item| {
            if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
                var.decls.iter().any(|d| {
                    if let ast::Pat::Ident(b) = &d.name {
                        b.id.sym.as_ref().starts_with("__cjs_")
                    } else {
                        false
                    }
                })
            } else {
                false
            }
        })
        .count();
    assert_eq!(
        cjs_let_count, 0,
        "non-string computed property should not produce __cjs_ let decl"
    );
}

#[test]
fn transform_expr_handles_binary() {
    let module = parse(r#"const x = require('./foo'); console.log(x + 1);"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_unary() {
    let module = parse(r#"const x = require('./foo'); console.log(-x);"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_update() {
    let module = parse(r#"let x = 1; x++; console.log(x);"#);
    let transformed = transform(&module);
    let has_var = transformed.body.iter().any(|item| {
        matches!(
            item,
            ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(_)))
        )
    });
    assert!(has_var);
}

#[test]
fn transform_expr_handles_conditional() {
    let module = parse(r#"const x = require('./foo'); console.log(x ? 1 : 2);"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_sequence() {
    let module = parse(r#"const x = require('./foo'); console.log((x, 1));"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_arrow_with_body() {
    let module = parse(r#"const x = require('./foo'); const fn = () => { return x; };"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_arrow_expr_body() {
    let module = parse(r#"const x = require('./foo'); const fn = () => x;"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_template() {
    let module = parse(r#"const x = require('./foo'); console.log(`${x}`);"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_new() {
    let module = parse(r#"const x = require('./foo'); console.log(new x());"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_paren() {
    let module = parse(r#"const x = require('./foo'); console.log((x));"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_object_spread() {
    let module = parse(r#"const x = require('./foo'); console.log({ ...x });"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_opt_chain_member() {
    let module = parse(r#"const x = require('./foo'); console.log(x?.y);"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_opt_chain_call() {
    let module = parse(r#"const x = require('./foo'); console.log(x?.());"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_await() {
    let module = parse(r#"async function f() { const x = require('./foo'); await x; }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_yield() {
    let module = parse(r#"function* f() { const x = require('./foo'); yield x; }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_fn_expr() {
    let module = parse(r#"const x = require('./foo'); const fn = function() { return x; };"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_class_expr() {
    let module = parse(r#"const x = require('./foo'); const c = class { m() { return x; } };"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_tagged_template() {
    let module = parse(
        r#"const x = require('./foo'); function tag(t, v) { return v; } console.log(tag`${x}`);"#,
    );
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_block() {
    let module = parse(r#"const x = require('./foo'); { console.log(x); }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_if() {
    let module = parse(r#"const x = require('./foo'); if (true) { console.log(x); }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_if_with_else() {
    let module = parse(
        r#"const x = require('./foo'); if (true) { console.log(x); } else { console.log(x); }"#,
    );
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_while() {
    let module = parse(r#"const x = require('./foo'); while (false) { console.log(x); }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_for() {
    let module =
        parse(r#"const x = require('./foo'); for (let i = 0; i < 1; i++) { console.log(x); }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_switch() {
    let module =
        parse(r#"const x = require('./foo'); switch (1) { case 1: console.log(x); break; }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_try_catch() {
    let module = parse(
        r#"const x = require('./foo'); try { console.log(x); } catch (e) { console.log(e); }"#,
    );
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_try_finally() {
    let module = parse(
        r#"const x = require('./foo'); try { console.log(x); } finally { console.log(x); }"#,
    );
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_return() {
    let module = parse(r#"function f() { const x = require('./foo'); return x; }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_throw() {
    let module = parse(r#"const x = require('./foo'); throw x;"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_labeled() {
    let module = parse(r#"const x = require('./foo'); label: { console.log(x); }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn var_decl_with_non_require_init_preserved() {
    let module = parse(r#"const x = 42; module.exports.foo = x;"#);
    let transformed = transform(&module);
    let has_const_x = transformed.body.iter().any(|item| {
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
            var.decls.iter().any(|d| {
                if let ast::Pat::Ident(b) = &d.name {
                    b.id.sym.as_ref() == "x"
                } else {
                    false
                }
            })
        } else {
            false
        }
    });
    assert!(has_const_x, "non-require var decl should be preserved");
}

#[test]
fn is_module_exports_member_returns_false_for_non_member() {
    assert!(!is_module_exports_member(&ast::Expr::Ident(
        ast::Ident::new("module".into(), DUMMY_SP, SyntaxContext::default(),)
    )));
}

#[test]
fn is_exports_member_returns_false_for_non_member() {
    assert!(!is_exports_member(&ast::Expr::Ident(ast::Ident::new(
        "exports".into(),
        DUMMY_SP,
        SyntaxContext::default(),
    ))));
}

#[test]
fn is_exports_ident_returns_false_for_non_ident() {
    assert!(!is_exports_ident(&ast::Expr::Lit(ast::Lit::Num(
        ast::Number {
            span: DUMMY_SP,
            value: 1.0,
            raw: None,
        }
    ))));
}

#[test]
fn is_module_exports_member_no_prop_returns_false_for_non_ident_obj() {
    let obj = ast::Expr::Lit(ast::Lit::Null(ast::Null { span: DUMMY_SP }));
    let prop = ast::MemberProp::Ident(ast::IdentName::new("exports".into(), DUMMY_SP));
    assert!(!is_module_exports_member_no_prop(&obj, &prop));
}

#[test]
fn is_module_exports_member_no_prop_returns_false_for_non_ident_prop() {
    let obj = ast::Expr::Ident(ast::Ident::new(
        "module".into(),
        DUMMY_SP,
        SyntaxContext::default(),
    ));
    let prop = ast::MemberProp::Computed(ast::ComputedPropName {
        span: DUMMY_SP,
        expr: Box::new(ast::Expr::Lit(ast::Lit::Str(ast::Str {
            span: DUMMY_SP,
            value: "exports".into(),
            raw: None,
        }))),
    });
    assert!(!is_module_exports_member_no_prop(&obj, &prop));
}

#[test]
fn is_module_exports_member_returns_false_for_wrong_obj_name() {
    let module = parse(r#"obj.exports.foo = 1;"#);
    assert!(!is_commonjs_module(&module));
}

#[test]
fn is_module_exports_member_returns_false_for_wrong_prop_name() {
    let module = parse(r#"module.other.foo = 1;"#);
    assert!(!is_commonjs_module(&module));
}

#[test]
fn transform_expr_handles_assign_pat_target() {
    let module = parse(r#"let x; ({x} = {x: 1}); console.log(x);"#);
    let transformed = transform(&module);
    let has_expr = transformed
        .body
        .iter()
        .any(|item| matches!(item, ast::ModuleItem::Stmt(ast::Stmt::Expr(_))));
    assert!(has_expr);
}

#[test]
fn transform_expr_handles_assign_simple_non_member() {
    let module = parse(r#"let x = 1; x = 2; console.log(x);"#);
    let transformed = transform(&module);
    assert!(!has_default_export(&transformed));
}

#[test]
fn transform_decl_handles_non_var() {
    let module = parse(r#"function foo() {} module.exports.bar = 1;"#);
    let transformed = transform(&module);
    let has_fn_decl = transformed.body.iter().any(|item| {
        matches!(
            item,
            ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(_)))
        )
    });
    assert!(has_fn_decl, "function declaration should be preserved");
}

#[test]
fn transform_var_decl_empty_after_removal() {
    let module = parse(r#"const lib = require('./lib'); console.log(lib);"#);
    let transformed = transform(&module);
    let has_empty_const = transformed.body.iter().any(|item| {
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
            var.kind == ast::VarDeclKind::Const && var.decls.is_empty()
        } else {
            false
        }
    });
    assert!(!has_empty_const, "empty var decls should be removed");
}

#[test]
fn transform_block_handles_export_decl_in_block() {
    let module = parse(r#"const x = require('./foo'); { console.log(x); }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_do_while() {
    let module = parse(r#"const x = require('./foo'); do { console.log(x); } while (false);"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_for_in() {
    let module = parse(r#"const x = require('./foo'); for (let k in {}) { console.log(x); }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_for_of() {
    let module = parse(r#"const x = require('./foo'); for (let v of []) { console.log(x); }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_stmt_handles_with() {
    let module = parse(r#"const x = require('./foo'); with ({}) { console.log(x); }"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_member() {
    let module = parse(r#"const x = require('./foo'); console.log(x.y);"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_assign_member() {
    let module = parse(r#"const x = require('./foo'); x.y = 1;"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_array() {
    let module = parse(r#"const x = require('./foo'); console.log([x]);"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_object() {
    let module = parse(r#"const x = require('./foo'); console.log({ y: x });"#);
    let transformed = transform(&module);
    assert!(has_import_decl(&transformed));
}

#[test]
fn transform_expr_handles_call_with_super() {
    let module = parse(r#"class A { constructor() { super(); } }"#);
    let transformed = transform(&module);
    let has_class = transformed.body.iter().any(|item| {
        matches!(
            item,
            ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Class(_)))
        )
    });
    assert!(has_class);
}

#[test]
fn require_in_fn_expr_body_is_transformed() {
    let module = parse(
        r#"
            const fn = function() {
                const x = require('./foo');
                return x;
            };
        "#,
    );
    let transformed = transform(&module);

    assert!(
        has_import_decl(&transformed),
        "should have import declaration"
    );

    let fn_body_ok = transformed.body.iter().any(|item| {
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
            for decl in &var.decls {
                if let ast::Pat::Ident(binding) = &decl.name {
                    if binding.id.sym == "fn" {
                        if let Some(ast::Expr::Fn(f)) = decl.init.as_deref() {
                            if let Some(body) = &f.function.body {
                                for stmt in &body.stmts {
                                    if let ast::Stmt::Decl(ast::Decl::Var(v)) = stmt {
                                        for d in &v.decls {
                                            if let ast::Pat::Ident(b) = &d.name {
                                                if b.id.sym == "x" {
                                                    if let Some(init) = &d.init {
                                                        if let ast::Expr::Call(call) =
                                                            init.as_ref()
                                                        {
                                                            if let ast::Callee::Expr(callee) =
                                                                &call.callee
                                                            {
                                                                if let ast::Expr::Ident(id) =
                                                                    callee.as_ref()
                                                                {
                                                                    if id.sym == "require" {
                                                                        return false;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    });
    assert!(
        fn_body_ok,
        "require() in function expression body should be transformed"
    );
}

#[test]
fn require_in_fn_decl_body_is_transformed() {
    let module = parse(
        r#"
            function fn() {
                const x = require('./foo');
                return x;
            }
        "#,
    );
    let transformed = transform(&module);
    assert!(
        has_import_decl(&transformed),
        "should have import declaration"
    );

    let fn_body_ok = transformed.body.iter().any(|item| {
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(fn_decl))) = item {
            if fn_decl.ident.sym == "fn" {
                if let Some(body) = &fn_decl.function.body {
                    for stmt in &body.stmts {
                        if let ast::Stmt::Decl(ast::Decl::Var(v)) = stmt {
                            for d in &v.decls {
                                if let ast::Pat::Ident(b) = &d.name {
                                    if b.id.sym == "x" {
                                        if let Some(init) = &d.init {
                                            if let ast::Expr::Call(call) = init.as_ref() {
                                                if let ast::Callee::Expr(callee) = &call.callee
                                                {
                                                    if let ast::Expr::Ident(id) =
                                                        callee.as_ref()
                                                    {
                                                        if id.sym == "require" {
                                                            return false;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    return true;
                }
            }
        }
        false
    });
    assert!(
        fn_body_ok,
        "require() in function declaration body should be transformed"
    );
}

#[test]
fn require_in_class_method_body_is_transformed() {
    let module = parse(
        r#"
            class MyClass {
                method() {
                    const x = require('./foo');
                    return x;
                }
            }
        "#,
    );
    let transformed = transform(&module);
    assert!(
        has_import_decl(&transformed),
        "should have import declaration"
    );

    let method_body_ok = transformed.body.iter().any(|item| {
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Class(class_decl))) = item {
            if class_decl.ident.sym == "MyClass" {
                for member in &class_decl.class.body {
                    if let ast::ClassMember::Method(method) = member {
                        if let Some(body) = &method.function.body {
                            for stmt in &body.stmts {
                                if let ast::Stmt::Decl(ast::Decl::Var(v)) = stmt {
                                    for d in &v.decls {
                                        if let ast::Pat::Ident(b) = &d.name {
                                            if b.id.sym == "x" {
                                                if let Some(init) = &d.init {
                                                    if let ast::Expr::Call(call) = init.as_ref()
                                                    {
                                                        if let ast::Callee::Expr(callee) =
                                                            &call.callee
                                                        {
                                                            if let ast::Expr::Ident(id) =
                                                                callee.as_ref()
                                                            {
                                                                if id.sym == "require" {
                                                                    return false;
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            return true;
                        }
                    }
                }
            }
        }
        false
    });
    assert!(
        method_body_ok,
        "require() in class method body should be transformed"
    );
}

#[test]
fn module_exports_default_and_named_both_exported() {
    let module = parse(
        r#"
            module.exports = function() { return 42; };
            module.exports.VERSION = '1.0';
        "#,
    );
    let transformed = transform(&module);

    let has_default = transformed.body.iter().any(|item| {
        matches!(
            item,
            ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultExpr(_))
        )
    });
    assert!(has_default, "should have default export");

    let has_named = transformed.body.iter().any(|item| {
        if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportNamed(named)) = item {
            named.specifiers.iter().any(|s| {
                if let ast::ExportSpecifier::Named(n) = s {
                    n.exported
                        .as_ref()
                        .map(|e| {
                            if let ast::ModuleExportName::Ident(id) = e {
                                id.sym == "VERSION"
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false)
                } else {
                    false
                }
            })
        } else {
            false
        }
    });
    assert!(has_named, "should have named export for VERSION");

    let has_version_var = transformed.body.iter().any(|item| {
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
            var.decls.iter().any(|d| {
                if let ast::Pat::Ident(b) = &d.name {
                    b.id.sym.contains("VERSION")
                } else {
                    false
                }
            })
        } else {
            false
        }
    });
    assert!(has_version_var, "VERSION variable should exist");
}
