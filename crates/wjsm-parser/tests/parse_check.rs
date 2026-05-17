#[test]
fn parse_x_newline_plusplus() {
    let code = "x = 1; x\u{000A}++";
    // Using script mode should catch this syntax error
    let result = wjsm_parser::parse_script_as_module(code);
    assert!(result.is_err(), "x\\n++ should fail to parse");
}
