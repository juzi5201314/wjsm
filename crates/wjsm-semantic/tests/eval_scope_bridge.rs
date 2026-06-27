#[test]
fn eval_scope_bridge_read_checks_exception() {
    let module = wjsm_parser::parse_script_as_module("var r = x;").unwrap();
    let program = wjsm_semantic::lower_eval_module_with_scope(module, true, true).unwrap();
    let dump = program.dump_text();
    assert!(dump.contains("call builtin.eval_get_binding"));
    assert!(dump.contains("is_exception"));
    assert!(dump.contains("throw"));
}
