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
fn unsupported_statement_reports_diagnostic() {
    let source = "function greet() {}\n";
    let error = lower_module(parse_module(source).expect("parse should succeed"))
        .expect_err("lowering should fail");

    match error {
        LoweringError::Diagnostic(diagnostic) => {
            assert!(
                diagnostic
                    .message
                    .contains("unsupported declaration kind `function`")
            );
            assert!(diagnostic.start < diagnostic.end);
        }
    }
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

fn assert_snapshot(name: &str) {
    let root = workspace_root();
    let source_path = root.join("fixtures/happy").join(format!("{name}.js"));
    let expected_path = root.join("fixtures/semantic").join(format!("{name}.ir"));

    let source = std::fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()));
    let expected = std::fs::read_to_string(&expected_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", expected_path.display()));

    let module = parse_module(&source).expect("fixture source should parse");
    let lowered = lower_module(module).expect("fixture lowering should succeed");

    assert_eq!(lowered.dump_text(), expected);
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve")
}
