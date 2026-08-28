//! sloppy 简单参数列表的重复形参（§15.2.1：非严格且 IsSimpleParameterList
//! 时允许，后者胜）降级正确性。

fn lower(source: &str) -> Result<wjsm_ir::Program, wjsm_semantic::LoweringError> {
    wjsm_semantic::lower_module(wjsm_parser::parse_module(source).unwrap(), false)
}

#[test]
fn sloppy_duplicate_params_lower_with_last_binding_winning() {
    let program = lower("function f(a, a) { return a; }\nconsole.log(f(1, 2));\n")
        .expect("sloppy 简单参数列表允许重复形参");
    let function = program
        .functions()
        .iter()
        .find(|function| function.name() == "f")
        .expect("函数 f 应存在");
    // $env、$this 之后两个形参槽位：前一个重命名为临时槽，最后一个持有真名。
    let params = function.params();
    assert_eq!(params.len(), 4);
    assert!(!params[2].ends_with(".a"), "前一次出现应重命名: {params:?}");
    assert!(
        params[3].ends_with(".a"),
        "最后一次出现持有真名: {params:?}"
    );
}

#[test]
fn function_expression_duplicate_params_lower() {
    lower("const f = function (a, a) { return a; };\nconsole.log(f(1, 2));\n")
        .expect("函数表达式的 sloppy 重复形参同样允许");
}

#[test]
fn strict_directive_rejects_duplicate_params() {
    assert!(lower("function f(a, a) { 'use strict'; return a; }\n").is_err());
}

#[test]
fn module_strict_rejects_duplicate_params() {
    assert!(lower("'use strict';\nfunction f(a, a) { return a; }\n").is_err());
}

#[test]
fn non_simple_parameter_list_rejects_duplicates() {
    assert!(lower("function f(a, a, b = 1) { return a; }\n").is_err());
    assert!(lower("function f(a, a, ...rest) { return a; }\n").is_err());
}

#[test]
fn arrow_params_reject_duplicates() {
    assert!(lower("const g = (a, a) => a;\n").is_err());
}
