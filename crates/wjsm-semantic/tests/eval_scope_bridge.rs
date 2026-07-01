#[test]
fn eval_scope_bridge_read_checks_exception() {
    let module = wjsm_parser::parse_script_as_module("var r = x;").unwrap();
    let program = wjsm_semantic::lower_eval_module_with_scope(module, true, true).unwrap();
    let dump = program.dump_text();
    assert!(dump.contains("call builtin.eval_get_binding"));
    assert!(dump.contains("is_exception"));
    assert!(dump.contains("throw"));
}

#[test]
fn eval_literal_binding_names_uses_parser_var_declared_names() {
    let names = wjsm_semantic::eval_literal_binding_names(
        r#"var s = "let phantom"; var { a: real, b: [nested] } = src; function named() {} class ignored {}"#,
    );
    assert_eq!(names, ["s", "real", "nested", "named"]);
}

#[test]
fn eval_literal_binding_names_ignores_lexical_declarations() {
    let names = wjsm_semantic::eval_literal_binding_names("let x = 1; const y = 2; class Z {}");
    assert!(names.is_empty());
}
