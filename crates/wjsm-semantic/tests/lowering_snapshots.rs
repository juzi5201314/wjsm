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
fn unsupported_statement_reports_diagnostic() {
    let source = "let value = 1;\nconsole.log(value);\n";
    let error = lower_module(parse_module(source).expect("parse should succeed"))
        .expect_err("lowering should fail");

    match error {
        LoweringError::Diagnostic(diagnostic) => {
            assert!(
                diagnostic
                    .message
                    .contains("unsupported statement kind `decl`")
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
