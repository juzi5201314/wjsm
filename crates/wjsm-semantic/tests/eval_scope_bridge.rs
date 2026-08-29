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

/// eval 桥下无捕获函数声明仍物化为携带词法环境的闭包（链根接调用方
/// ScopeRecord），嵌套函数自由名的 EvalGetBinding 沿链解析到调用方绑定。
#[test]
fn eval_captureless_fn_decl_materializes_bridge_closure() {
    let dump = lower_eval_dump("function o(){ function i(){ return x; } return i(); } o();");
    assert!(dump.contains("call builtin.create_closure"));
    assert!(dump.contains("call builtin.eval_get_binding"));
}

/// 生成器 body 的 `$env` 槽位持有续体对象：eval 桥读自由名必须经续体槽
/// 还原的 `$closure_env`（wrapper 词法环境），不得把续体当环境解析。
#[test]
fn eval_generator_body_bridge_env_uses_closure_env() {
    let dump = lower_eval_dump("function* g(){ yield x; } g().next();");
    let body = dump
        .split("function ")
        .find(|section| section.contains("call builtin.eval_get_binding"))
        .expect("生成器 body 应含 eval_get_binding");
    assert!(body.contains(".$closure_env"));
}

/// 嵌套 direct eval 站点保存并恢复 `$eval_env` 协议槽（image 级共享，
/// 覆写后外层 eval 体的后续自由名解析会读到已销毁的内层记录），并把
/// 新记录的 outer 接当前桥环境（meta key 4，共 5 处 set_meta）。
#[test]
fn eval_nested_direct_eval_restores_protocol_slot_and_chains_outer() {
    let dump = lower_eval_dump("eval('1'); x;");
    let store_count = dump.matches("store var $eval_env").count();
    assert!(store_count >= 2, "记录写入 + 槽恢复，实际 {store_count}");
    let meta_count = dump.matches("call builtin.scope_record_set_meta").count();
    assert_eq!(
        meta_count, 5,
        "strict/args/super/new_target/outer 五项 meta"
    );
}

/// 匿名生成器表达式经编译器内部临时名（`$__wjsm_*`）走声明存储路径：
/// 临时名不属于 eval 源码的 VarDeclaredNames，不得外泄到调用方记录。
#[test]
fn eval_anonymous_generator_expression_temp_name_does_not_leak() {
    let dump = lower_eval_dump("(function*(){ yield 1; });");
    assert!(!dump.contains("call builtin.eval_set_binding"));
}
