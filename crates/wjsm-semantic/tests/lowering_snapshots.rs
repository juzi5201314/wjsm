use std::path::{Path, PathBuf};

use wjsm_parser::parse_module;
use wjsm_semantic::{LoweringError, lower_module};

#[test]
fn hello_fixture_matches_ir_snapshot() {
    assert_snapshot("hello");
}

#[test]
fn arithmetic_fixture_matches_ir_snapshot() {
    assert_snapshot("arithmetic");
}

#[test]
fn let_decl_fixture_matches_ir_snapshot() {
    assert_snapshot("let_decl");
}

#[test]
fn block_scope_fixture_matches_ir_snapshot() {
    assert_snapshot("block_scope");
}

#[test]
fn assignment_fixture_matches_ir_snapshot() {
    assert_snapshot("assignment");
}

#[test]
fn compound_assign_fixture_matches_ir_snapshot() {
    assert_snapshot("compound_assign");
}

#[test]
fn compound_assign_nested_fixture_matches_ir_snapshot() {
    assert_snapshot("compound_assign_nested");
}

#[test]
fn var_hoist_fixture_matches_ir_snapshot() {
    assert_snapshot("var_hoist");
}

#[test]
fn var_hoist_read_before_decl_fixture_matches_ir_snapshot() {
    assert_snapshot("var_hoist_read_before_decl");
}

#[test]
fn var_no_init_redeclare_fixture_matches_ir_snapshot() {
    assert_snapshot("var_no_init_redeclare");
}

#[test]
fn block_var_hoist_before_block_fixture_matches_ir_snapshot() {
    assert_snapshot("block_var_hoist_before_block");
}

#[test]
fn undeclared_var_reports_diagnostic() {
    let source = "console.log(z);\n";
    let error = lower_module(parse_module(source).expect("parse should succeed"))
        .expect_err("lowering should fail");

    match error {
        LoweringError::Diagnostic(diagnostic) => {
            assert!(diagnostic.message.contains("undeclared identifier"));
            assert!(diagnostic.start < diagnostic.end);
        }
    }
}

#[test]
fn const_reassign_reports_diagnostic() {
    let source = "const x = 1; x = 2;\n";
    let error = lower_module(parse_module(source).expect("parse should succeed"))
        .expect_err("lowering should fail");

    match error {
        LoweringError::Diagnostic(diagnostic) => {
            assert!(
                diagnostic
                    .message
                    .contains("cannot reassign a const-declared variable")
            );
        }
    }
}

#[test]
fn tdz_access_reports_diagnostic() {
    let source = "{ console.log(x); let x = 1; }\n";
    let error = lower_module(parse_module(source).expect("parse should succeed"))
        .expect_err("lowering should fail");

    match error {
        LoweringError::Diagnostic(diagnostic) => {
            assert!(
                diagnostic
                    .message
                    .contains("cannot access `x` before initialisation")
            );
        }
    }
}

#[test]
fn let_redeclare_reports_diagnostic() {
    let source = "let x = 1; let x = 2;\n";
    let error = lower_module(parse_module(source).expect("parse should succeed"))
        .expect_err("lowering should fail");

    match error {
        LoweringError::Diagnostic(diagnostic) => {
            assert!(diagnostic.message.contains("cannot redeclare identifier"));
        }
    }
}
#[test]
fn function_decl_is_supported() {
    let source = "function greet() {}\n";
    let result = lower_module(parse_module(source).expect("parse should succeed"));
    assert!(result.is_ok(), "function declarations should be supported");
    let program = result.unwrap();
    let text = program.dump_text();
    assert!(text.contains("fn @greet"), "should have a 'greet' function");
    assert!(text.contains("fn @main"), "should still have main");
    assert!(text.contains("functionref(@0)"), "should reference greet");
    assert!(
        text.contains("store var $0.greet"),
        "should store greet in module scope"
    );
}

#[test]
fn console_log_without_arguments_reports_diagnostic() {
    let source = "console.log();\n";
    let error = lower_module(parse_module(source).expect("parse should succeed"))
        .expect_err("lowering should fail");

    match error {
        LoweringError::Diagnostic(diagnostic) => {
            assert!(
                diagnostic
                    .message
                    .contains("console.log requires at least 1 argument")
            );
        }
    }
}

#[test]
fn if_else_fixture_matches_ir_snapshot() {
    assert_snapshot("if_else");
}

#[test]
fn comparison_fixture_matches_ir_snapshot() {
    assert_snapshot("comparison");
}

#[test]
fn bool_null_fixture_matches_ir_snapshot() {
    assert_snapshot("bool_null");
}

#[test]
fn while_count_fixture_matches_ir_snapshot() {
    assert_snapshot("while_count");
}

#[test]
fn do_while_once_fixture_matches_ir_snapshot() {
    assert_snapshot("do_while_once");
}

#[test]
fn for_sum_fixture_matches_ir_snapshot() {
    assert_snapshot("for_sum");
}

#[test]
fn break_continue_fixture_matches_ir_snapshot() {
    assert_snapshot("break_continue");
}

#[test]
fn return_early_fixture_matches_ir_snapshot() {
    assert_snapshot("return_early");
}

#[test]
fn switch_basic_fixture_matches_ir_snapshot() {
    assert_snapshot("switch_basic");
}

#[test]
fn switch_fallthrough_fixture_matches_ir_snapshot() {
    assert_snapshot("switch_fallthrough");
}

#[test]
fn switch_default_middle_fixture_matches_ir_snapshot() {
    assert_snapshot("switch_default_middle");
}

#[test]
fn switch_in_loop_continue_fixture_matches_ir_snapshot() {
    assert_snapshot("switch_in_loop_continue");
}

#[test]
fn switch_with_let_fixture_matches_ir_snapshot() {
    assert_snapshot("switch_with_let");
}

#[test]
fn try_catch_fixture_matches_ir_snapshot() {
    assert_snapshot("try_catch");
}

#[test]
fn try_finally_fixture_matches_ir_snapshot() {
    assert_snapshot("try_finally");
}

#[test]
fn try_catch_finally_fixture_matches_ir_snapshot() {
    assert_snapshot("try_catch_finally");
}

#[test]
fn throw_uncaught_fixture_matches_ir_snapshot() {
    assert_snapshot("throw_uncaught");
}

#[test]
fn try_finally_nested_fixture_matches_ir_snapshot() {
    assert_snapshot("try_finally_nested");
}

#[test]
fn for_in_string_fixture_matches_ir_snapshot() {
    assert_snapshot("for_in_string");
}

#[test]
fn for_of_string_fixture_matches_ir_snapshot() {
    assert_snapshot("for_of_string");
}

#[test]
fn for_of_nested_break_continue_fixture_matches_ir_snapshot() {
    assert_snapshot("for_of_nested_break_continue");
}

#[test]
fn empty_debugger_fixture_matches_ir_snapshot() {
    assert_snapshot("empty_debugger");
}

#[test]
fn logical_and_or_fixture_matches_ir_snapshot() {
    assert_snapshot("logical_and_or");
}

#[test]
fn nullish_fixture_matches_ir_snapshot() {
    assert_snapshot("nullish");
}

#[test]
fn ternary_phi_fixture_matches_ir_snapshot() {
    assert_snapshot("ternary_phi");
}

#[test]
fn labeled_fixture_matches_ir_snapshot() {
    assert_snapshot("labeled");
}

#[test]
fn ternary_nested_fixture_matches_ir_snapshot() {
    assert_snapshot("ternary_nested");
}

#[test]
fn empty_string_truthy_fixture_matches_ir_snapshot() {
    assert_snapshot("empty_string_truthy");
}

#[test]
fn try_finally_throw_fixture_matches_ir_snapshot() {
    assert_snapshot("try_finally_throw");
}

#[test]
fn try_finally_return_fixture_matches_ir_snapshot() {
    assert_snapshot("try_finally_return");
}

#[test]
fn switch_default_fallthrough_fixture_matches_ir_snapshot() {
    assert_snapshot("switch_default_fallthrough");
}

#[test]
fn switch_if_else_fixture_matches_ir_snapshot() {
    assert_snapshot("switch_if_else");
}

#[test]
fn switch_while_fixture_matches_ir_snapshot() {
    assert_snapshot("switch_while");
}

#[test]
fn compound_assign_ext_fixture_matches_ir_snapshot() {
    assert_snapshot("compound_assign_ext");
}

#[test]
fn logical_compound_assign_fixture_matches_ir_snapshot() {
    assert_snapshot("logical_compound_assign");
}

#[test]
fn update_fixture_matches_ir_snapshot() {
    assert_snapshot("update");
}

#[test]
fn array_proto_call_fixture_matches_ir_snapshot() {
    assert_snapshot("array_proto_call");
}

#[test]
fn array_proto_filter_fixture_matches_ir_snapshot() {
    assert_snapshot("array_proto_filter");
}

#[test]
fn template_string_fixture_matches_ir_snapshot() {
    assert_snapshot("template_string");
}

#[test]
fn tagged_template_fixture_matches_ir_snapshot() {
    assert_snapshot("tagged_template");
}

fn assert_snapshot(name: &str) {
    let root = workspace_root();
    let source_path = root.join("fixtures/happy").join(format!("{name}.js"));
    let expected_path = root.join("fixtures/semantic").join(format!("{name}.ir"));

    let source = std::fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()));

    let module = parse_module(&source).expect("fixture source should parse");
    let lowered = lower_module(module).expect("fixture lowering should succeed");
    let actual = lowered.dump_text();

    if std::env::var("WJSM_UPDATE_SNAPSHOTS").is_ok() {
        std::fs::write(&expected_path, &actual)
            .unwrap_or_else(|error| panic!("failed to write {}: {error}", expected_path.display()));
        return;
    }

    let expected = std::fs::read_to_string(&expected_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", expected_path.display()));

    assert_eq!(actual, expected);
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve")
}
