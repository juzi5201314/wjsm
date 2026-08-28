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

fn lower_eval_dump(source: &str) -> String {
    let module = wjsm_parser::parse_script_as_module(source).unwrap();
    wjsm_semantic::lower_eval_module_with_scope(module, true, true)
        .unwrap()
        .dump_text()
}

/// eval 顶层 var 声明写回作用域记录（EvalDeclarationInstantiation 的
/// varEnv 是 eval 的变量环境）。
#[test]
fn eval_top_level_var_writes_to_scope_record() {
    let dump = lower_eval_dump("var top = 1;");
    assert!(dump.contains("call builtin.eval_set_binding"));
}

/// eval 代码内定义的函数自带 VariableEnvironment：函数体内的 var 是局部
/// 绑定，不得发射 eval_set_binding 外泄到全局/调用方。
#[test]
fn eval_nested_function_var_stays_local() {
    let dump = lower_eval_dump("(function(){ var q = 5; })();");
    assert!(!dump.contains("call builtin.eval_set_binding"));
}

/// eval 代码内定义的非箭头函数拥有自己的 new.target：经运行时 activation
/// （builtin.new_target）解析，而不是从调用方作用域记录读取。
#[test]
fn eval_nested_function_new_target_uses_activation() {
    let dump = lower_eval_dump("(function(){ return new.target; })();");
    assert!(dump.contains("call builtin.new.target"));
    assert!(!dump.contains("__wjsm_new_target"));
}

/// 函数内的箭头继承最近非箭头函数的 new.target：同样走运行时 activation。
#[test]
fn eval_arrow_in_function_new_target_uses_activation() {
    let dump = lower_eval_dump("(function(){ const a = () => new.target; return a(); })();");
    assert!(dump.contains("call builtin.new.target"));
    assert!(!dump.contains("__wjsm_new_target"));
}

/// eval 顶层的 new.target 引用调用方语境：仍经作用域记录读取。
#[test]
fn eval_top_level_new_target_reads_scope_record() {
    let dump = lower_eval_dump("new.target;");
    assert!(dump.contains("__wjsm_new_target"));
    assert!(!dump.contains("call builtin.new.target"));
}
