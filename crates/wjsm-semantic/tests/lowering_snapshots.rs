use std::path::{Path, PathBuf};

use wjsm_parser::parse_module;
use wjsm_semantic::{
    LoweringError, ModuleKind, ModuleLoweringInput, ModuleMetadata, lower_module, lower_modules,
};

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
    let error = lower_module(parse_module(source).expect("parse should succeed"), false)
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
    let error = lower_module(parse_module(source).expect("parse should succeed"), false)
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
    let error = lower_module(parse_module(source).expect("parse should succeed"), false)
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
    let error = lower_module(parse_module(source).expect("parse should succeed"), false)
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
    let result = lower_module(parse_module(source).expect("parse should succeed"), false);
    assert!(result.is_ok(), "function declarations should be supported");
    let program = result.unwrap();
    let text = program.dump_text();
    assert!(text.contains("fn @greet"), "should have a 'greet' function");
    assert!(
        text.contains("fn @$module_main"),
        "should still have module entry"
    );
    assert!(text.contains("functionref(@0)"), "should reference greet");
    assert!(
        text.contains("store var $0.greet"),
        "should store greet in module scope"
    );
}

#[test]
fn console_log_without_arguments_reports_diagnostic() {
    let source = "console.log();\n";
    let error = lower_module(parse_module(source).expect("parse should succeed"), false)
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
fn switch_nonliteral_fixture_matches_ir_snapshot() {
    assert_snapshot("switch_nonliteral");
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
fn try_finally_no_catch_fixture_matches_ir_snapshot() {
    assert_snapshot("try_finally_no_catch");
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

// ── async function ──────────────────────────────────────────────────────

#[test]
fn async_basic_fixture_matches_ir_snapshot() {
    assert_snapshot("async_basic");
}

#[test]
fn async_expr_fixture_matches_ir_snapshot() {
    assert_snapshot("async_expr");
}

#[test]
fn async_arrow_fixture_matches_ir_snapshot() {
    assert_snapshot("async_arrow");
}

#[test]
fn async_await_fixture_matches_ir_snapshot() {
    assert_snapshot("async_await");
}

#[test]
fn async_catch_fixture_matches_ir_snapshot() {
    assert_snapshot("async_catch");
}

#[test]
fn async_error_propagation_fixture_matches_ir_snapshot() {
    assert_snapshot("async_error_propagation");
}

#[test]
fn async_multi_await_fixture_matches_ir_snapshot() {
    assert_snapshot("async_multi_await");
}

#[test]
fn async_nested_fixture_matches_ir_snapshot() {
    assert_snapshot("async_nested");
}

#[test]
fn async_params_fixture_matches_ir_snapshot() {
    assert_snapshot("async_params");
}

#[test]
fn async_return_thenable_fixture_matches_ir_snapshot() {
    assert_snapshot("async_return_thenable");
}

#[test]
fn async_side_effect_fixture_matches_ir_snapshot() {
    assert_snapshot("async_side_effect");
}

#[test]
fn async_await_try_finally_fixture_matches_ir_snapshot() {
    assert_snapshot("async_await_try_finally");
}

// ── async generator ────────────────────────────────────────────────────

#[test]
fn async_generator_basic_fixture_matches_ir_snapshot() {
    assert_snapshot("async_generator_basic");
}

#[test]
fn async_generator_await_fixture_matches_ir_snapshot() {
    assert_snapshot("async_generator_await");
}

#[test]
fn async_generator_return_fixture_matches_ir_snapshot() {
    assert_snapshot("async_generator_return");
}

#[test]
fn for_await_async_generator_fixture_matches_ir_snapshot() {
    assert_snapshot("for_await_async_generator");
}

// ── Promise ────────────────────────────────────────────────────────────

#[test]
fn promise_chain_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_chain");
}

#[test]
fn promise_combinators_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_combinators");
}

#[test]
fn promise_all_values_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_all_values");
}

#[test]
fn promise_all_empty_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_all_empty");
}

#[test]
fn promise_all_pending_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_all_pending");
}

#[test]
fn promise_all_pending_reject_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_all_pending_reject");
}

#[test]
fn promise_all_settled_values_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_all_settled_values");
}

#[test]
fn promise_all_settled_pending_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_all_settled_pending");
}

#[test]
fn promise_any_values_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_any_values");
}

#[test]
fn promise_any_pending_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_any_pending");
}

#[test]
fn promise_race_values_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_race_values");
}

#[test]
fn promise_race_pending_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_race_pending");
}

#[test]
fn promise_constructor_resolver_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_constructor_resolver");
}

#[test]
fn promise_resolve_identity_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_resolve_identity");
}

#[test]
fn promise_resolve_thenable_microtask_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_resolve_thenable_microtask");
}

#[test]
fn promise_resolver_idempotence_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_resolver_idempotence");
}

#[test]
fn promise_thenable_assimilation_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_thenable_assimilation");
}

#[test]
fn promise_microtask_order_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_microtask_order");
}

#[test]
fn promise_finally_preserves_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_finally_preserves");
}

#[test]
fn promise_with_resolvers_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_with_resolvers");
}

// ── eval ───────────────────────────────────────────────────────────────

#[test]
fn eval_basic_fixture_matches_ir_snapshot() {
    assert_snapshot("eval_basic");
}

#[test]
fn eval_direct_assign_fixture_matches_ir_snapshot() {
    assert_snapshot("eval_direct_assign");
}

#[test]
fn eval_strict_existing_var_fixture_matches_ir_snapshot() {
    assert_snapshot("eval_strict_existing_var");
}

// ── async edge cases ───────────────────────────────────────────────────

#[test]
fn async_nested_chain_fixture_matches_ir_snapshot() {
    assert_snapshot("async_nested_chain");
}

#[test]
fn await_conditional_fixture_matches_ir_snapshot() {
    assert_snapshot("await_conditional");
}

#[test]
fn promise_value_coercion_fixture_matches_ir_snapshot() {
    assert_snapshot("promise_value_coercion");
}

#[test]
fn async_as_callback_fixture_matches_ir_snapshot() {
    assert_snapshot("async_as_callback");
}

#[test]
fn async_closure_capture_fixture_matches_ir_snapshot() {
    assert_snapshot("async_closure_capture");
}

// ── TS/TSX snapshot tests ─────────────────────────────────────

#[test]
fn proxy_basic_fixture_matches_ir_snapshot() {
    assert_snapshot("proxy_basic");
}
#[test]
fn proxy_get_trap_fixture_matches_ir_snapshot() {
    assert_snapshot("proxy_get_trap");
}
#[test]
fn proxy_set_trap_fixture_matches_ir_snapshot() {
    assert_snapshot("proxy_set_trap");
}
#[test]
fn proxy_has_trap_fixture_matches_ir_snapshot() {
    assert_snapshot("proxy_has_trap");
}
#[test]
fn proxy_delete_trap_fixture_matches_ir_snapshot() {
    assert_snapshot("proxy_delete_trap");
}
#[test]
fn proxy_apply_trap_fixture_matches_ir_snapshot() {
    assert_snapshot("proxy_apply_trap");
}
#[test]
fn proxy_revocable_fixture_matches_ir_snapshot() {
    assert_snapshot("proxy_revocable");
}
#[test]
fn reflect_methods_fixture_matches_ir_snapshot() {
    assert_snapshot("reflect_methods");
}
#[test]
fn ts_enum_fixture_matches_ir_snapshot() {
    assert_snapshot("ts_enum");
}
#[test]
fn ts_enum_reverse_fixture_matches_ir_snapshot() {
    assert_snapshot("ts_enum_reverse");
}
#[test]
fn ts_enum_reverse2_fixture_matches_ir_snapshot() {
    assert_snapshot("ts_enum_reverse2");
}
#[test]
fn ts_interface_fixture_matches_ir_snapshot() {
    assert_snapshot("ts_interface");
}
#[test]
fn ts_namespace_fixture_matches_ir_snapshot() {
    assert_snapshot("ts_namespace");
}
#[test]
fn ts_type_alias_fixture_matches_ir_snapshot() {
    assert_snapshot("ts_type_alias");
}
#[test]
fn ts_type_assertions_fixture_matches_ir_snapshot() {
    assert_snapshot("ts_type_assertions");
}
#[test]
fn using_basic_fixture_matches_ir_snapshot() {
    assert_snapshot("using_basic");
}
#[test]
fn using_block_scope_fixture_matches_ir_snapshot() {
    assert_snapshot("using_block_scope");
}
#[test]
fn jsx_basic_fixture_matches_ir_snapshot() {
    assert_snapshot("jsx_basic");
}
#[test]
fn jsx_attrs_fixture_matches_ir_snapshot() {
    assert_snapshot("jsx_attrs");
}
#[test]
fn jsx_expr_fixture_matches_ir_snapshot() {
    assert_snapshot("jsx_expr");
}
#[test]
fn jsx_fragment_fixture_matches_ir_snapshot() {
    assert_snapshot("jsx_fragment");
}

#[test]
fn sync_generator_basic_fixture_matches_ir_snapshot() {
    assert_snapshot("sync_generator_basic");
}

#[test]
fn method_closure_live_bindings_fixture_matches_ir_snapshot() {
    assert_snapshot("method_closure_live_bindings");
}

#[test]
fn class_private_closure_identity_fixture_matches_ir_snapshot() {
    assert_snapshot("class_private_closure_identity");
}

fn assert_snapshot(name: &str) {
    let root = workspace_root();
    let expected_path = root.join("fixtures/semantic").join(format!("{name}.ir"));

    // 依次尝试 .js / .ts / .tsx
    let source_dir = root.join("fixtures/happy");
    let source_path = [".js", ".ts", ".tsx"]
        .iter()
        .map(|ext| source_dir.join(format!("{name}{ext}")))
        .find(|p| p.exists())
        .unwrap_or_else(|| panic!("no source file (js/ts/tsx) found for {name}"));

    let source = std::fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()));

    let module = parse_module(&source).expect("fixture source should parse");
    let lowered = lower_module(module, false).expect("fixture lowering should succeed");
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

#[test]
fn eval_predeclare_function_name() {
    // Test that eval('function fun() {}') predeclares fun in non-strict mode
    let source = "let typeofInside; function outer() { eval('function fun() {}'); typeofInside = typeof fun; }\n";
    let result = lower_module(
        wjsm_parser::parse_module(source).expect("parse should succeed"),
        false,
    );
    assert!(
        result.is_ok(),
        "lowering should succeed, got: {:?}",
        result.err()
    );
}

// ── arguments-object lazy elision ────────────────────────────────────────
//
// A function whose body never references `arguments` must NOT materialise the
// implicit mapped arguments object (`collect_rest_args` + `create_mapped_arguments_object`,
// both may-GC). Eliding it restores ordinary functions to no-GC and unlocks the
// Layer 3 backend call-spill omission. The marker we assert on is the
// `create_mapped_arguments_object` builtin call in the IR dump.

const ARGS_OBJ_MARKER: &str = "create_mapped_arguments_object";

fn dump(source: &str) -> String {
    let module = wjsm_parser::parse_module(source).expect("source should parse");
    lower_module(module, false)
        .expect("lowering should succeed")
        .dump_text()
}

#[test]
fn require_runtime_cjs_scope_uses_module_local_binding() {
    let ast = parse_module("if (false) require('./never.js');\nconsole.log(typeof require, __filename, __dirname, module.exports === exports);\n")
        .expect("source should parse");
    let program = lower_modules(
        vec![ModuleLoweringInput {
            id: wjsm_ir::ModuleId(0),
            ast,
            metadata: ModuleMetadata {
                filename: "/project/main.cjs".to_string(),
                dirname: "/project".to_string(),
                url: "file:///project/main.cjs".to_string(),
                kind: ModuleKind::CommonJs,
            },
            source: None,
        }],
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
    )
    .expect("CJS runtime require lowering should succeed");
    let text = program.dump_text();

    assert!(
        text.contains("cjs.create_require"),
        "missing CJS require setup:\n{text}"
    );
    assert!(
        text.contains("cjs.register_module"),
        "missing CJS module registration:\n{text}"
    );
    assert!(
        !text.contains("globalThis.require"),
        "CJS lowering must not depend on the retired global bridge:\n{text}"
    );
}

fn esm_input(id: u32, filename: &str, source: &str) -> ModuleLoweringInput {
    let dirname = filename
        .rsplit_once('/')
        .map(|(dirname, _)| dirname)
        .unwrap_or("/project");
    ModuleLoweringInput {
        id: wjsm_ir::ModuleId(id),
        ast: parse_module(source).expect("source should parse"),
        metadata: ModuleMetadata {
            filename: filename.to_string(),
            dirname: dirname.to_string(),
            url: format!("file://{filename}"),
            kind: ModuleKind::Esm,
        },
        source: Some(std::sync::Arc::<str>::from(source)),
    }
}

fn lower_single_esm_source(source: &str) -> String {
    lower_modules(
        vec![esm_input(0, "/project/main.js", source)],
        &std::collections::HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>::new(),
        &std::collections::HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>::new(),
        &std::collections::HashMap::<wjsm_ir::ModuleId, std::collections::BTreeSet<String>>::new(),
        &std::collections::HashMap::<wjsm_ir::ModuleId, Vec<(String, wjsm_ir::ModuleId)>>::new(),
        &std::collections::HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ReExportBinding>>::new(),
    )
    .expect("ESM lowering should succeed")
    .dump_text()
}

#[test]
fn module_scope_binding_survives_top_level_await() {
    let text = lower_single_esm_source(
        "const later = () => 42; await Promise.resolve(); console.log(later());\n",
    );

    assert!(
        text.contains("continuation.save_var") && text.matches("store var $1.later").count() >= 2,
        "module binding must be saved and restored around top-level await:\n{text}"
    );
}

fn lower_single_esm_error(source: &str) -> LoweringError {
    lower_modules(
        vec![esm_input(0, "/project/main.js", source)],
        &std::collections::HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>::new(),
        &std::collections::HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>::new(),
        &std::collections::HashMap::<wjsm_ir::ModuleId, std::collections::BTreeSet<String>>::new(),
        &std::collections::HashMap::<wjsm_ir::ModuleId, Vec<(String, wjsm_ir::ModuleId)>>::new(),
        &std::collections::HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ReExportBinding>>::new(),
    )
    .expect_err("ESM lowering should reject this source")
}

fn assert_unsupported_dynamic_import_extra_arg(error: LoweringError) {
    match error {
        LoweringError::Diagnostic(diagnostic) => {
            assert!(
                diagnostic
                    .message
                    .contains("import() currently supports only the module specifier argument"),
                "unexpected diagnostic: {}",
                diagnostic.message
            );
        }
    }
}

#[test]
fn dynamic_import_expression_lowers_to_runtime_host_path() {
    let text = lower_single_esm_source("const path = './dep.js'; import(path);\n");

    assert!(
        text.contains("dynamic_import_runtime"),
        "dynamic import expression should lower to runtime host path:\n{text}"
    );
}

#[test]
fn dynamic_import_template_expression_lowers_to_runtime_host_path() {
    let text = lower_single_esm_source("const name = 'dep'; import(`./${name}.js`);\n");

    assert!(
        text.contains("dynamic_import_runtime"),
        "template dynamic import should lower to runtime host path:\n{text}"
    );
}

#[test]
fn dynamic_import_json_parse_abrupt_lowers_to_runtime_rejection_path() {
    let text = lower_single_esm_source("import(JSON.parse('bad'));\n");

    assert!(
        text.contains("dynamic_import_runtime"),
        "JSON.parse specifier abrupt should still reach dynamic import runtime path:\n{text}"
    );
    assert!(
        text.matches("dynamic_import_runtime").count() >= 2,
        "specifier abrupt branch should call dynamic import runtime with the original exception:\n{text}"
    );
    if let Some(exception_value_pos) = text.find("exception_value") {
        let runtime_pos = text.find("dynamic_import_runtime").unwrap();
        assert!(
            exception_value_pos > runtime_pos,
            "specifier abrupt must not be unwrapped before dynamic import runtime owns rejection:\n{text}"
        );
    }
}

#[test]
fn dynamic_import_import_meta_resolve_abrupt_lowers_to_runtime_rejection_path() {
    let text = lower_single_esm_source("import(import.meta.resolve('./missing.js'));\n");

    assert!(
        text.contains("import_meta.resolve") && text.contains("dynamic_import_runtime"),
        "import.meta.resolve specifier abrupt should be passed to dynamic import runtime:\n{text}"
    );
    assert!(
        text.matches("dynamic_import_runtime").count() >= 2,
        "import.meta.resolve abrupt branch should call dynamic import runtime with the original exception:\n{text}"
    );
    if let Some(exception_value_pos) = text.find("exception_value") {
        let runtime_pos = text.find("dynamic_import_runtime").unwrap();
        assert!(
            exception_value_pos > runtime_pos,
            "import.meta.resolve abrupt must not be unwrapped before dynamic import runtime owns rejection:\n{text}"
        );
    }
}

#[test]
fn dynamic_import_composed_json_parse_abrupt_lowers_to_runtime_rejection_path() {
    let text = lower_single_esm_source("import(JSON.parse('bad') + './never.js');\n");

    assert!(
        text.contains("JSON.parse") && text.contains("dynamic_import_runtime"),
        "composed JSON.parse specifier abrupt should still reach dynamic import runtime path:\n{text}"
    );
    assert!(
        text.matches("dynamic_import_runtime").count() >= 2,
        "composed specifier abrupt branch should call dynamic import runtime before stringification:\n{text}"
    );
}

#[test]
fn dynamic_import_conditional_json_parse_abrupt_lowers_to_runtime_rejection_path() {
    let text = lower_single_esm_source("import((true ? JSON.parse('bad') : './dep.js') + '?x');\n");

    assert!(
        text.contains("JSON.parse") && text.contains("dynamic_import_runtime"),
        "conditional JSON.parse specifier abrupt should still reach dynamic import runtime path:\n{text}"
    );
    assert!(
        text.matches("dynamic_import_runtime").count() >= 2,
        "conditional specifier abrupt branch should call dynamic import runtime before stringification:\n{text}"
    );
}

#[test]
fn dynamic_import_sequence_json_parse_abrupt_lowers_to_runtime_rejection_path() {
    let text = lower_single_esm_source("import((JSON.parse('bad'), './dep.js'));\n");

    assert!(
        text.contains("JSON.parse") && text.contains("dynamic_import_runtime"),
        "sequence JSON.parse specifier abrupt should still reach dynamic import runtime path:\n{text}"
    );
    assert!(
        text.matches("dynamic_import_runtime").count() >= 2,
        "sequence abrupt branch should pass the original exception to dynamic import runtime before the final specifier can overwrite it:\n{text}"
    );
}

#[test]
fn dynamic_import_sequence_normal_completion_lowers_final_specifier_path() {
    let text = lower_single_esm_source(
        "function sideEffect() { return 1; } import((sideEffect(), './dep.js'));\n",
    );

    assert!(
        text.contains("sideEffect")
            && text.contains("./dep.js")
            && text.contains("dynamic_import_runtime"),
        "normal sequence specifier should keep evaluating to the final specifier on the runtime path:\n{text}"
    );
}

#[test]
fn dynamic_import_composed_import_meta_resolve_abrupt_lowers_to_runtime_rejection_path() {
    let text = lower_single_esm_source("import(import.meta.resolve('./missing.js') + '?x');\n");

    assert!(
        text.contains("import_meta.resolve") && text.contains("dynamic_import_runtime"),
        "composed import.meta.resolve specifier abrupt should reach dynamic import runtime path:\n{text}"
    );
    assert!(
        text.matches("dynamic_import_runtime").count() >= 2,
        "composed import.meta.resolve abrupt branch should call dynamic import runtime before stringification:\n{text}"
    );
}

#[test]
fn dynamic_import_expression_extra_arg_reports_unsupported() {
    let error = lower_single_esm_error(
        "const path = './dep.js'; import(path, { with: { type: 'json' } });\n",
    );

    assert_unsupported_dynamic_import_extra_arg(error);
}

#[test]
fn dynamic_import_static_literal_keeps_static_fast_path() {
    let mut dynamic_targets = std::collections::HashMap::new();
    dynamic_targets.insert(wjsm_ir::ModuleId(0), vec![wjsm_ir::ModuleId(1)]);
    let mut dynamic_specifiers = std::collections::HashMap::new();
    dynamic_specifiers.insert(
        wjsm_ir::ModuleId(0),
        vec![("./dep.js".to_string(), wjsm_ir::ModuleId(1))],
    );
    let mut export_names = std::collections::HashMap::new();
    export_names.insert(
        wjsm_ir::ModuleId(1),
        std::collections::BTreeSet::from(["value".to_string()]),
    );
    let program = lower_modules(
        vec![
            esm_input(0, "/project/main.js", "import('./dep.js');\n"),
            esm_input(1, "/project/dep.js", "export const value = 1;\n"),
        ],
        &std::collections::HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>::new(),
        &dynamic_targets,
        &export_names,
        &dynamic_specifiers,
        &std::collections::HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ReExportBinding>>::new(),
    )
    .expect("static dynamic import lowering should succeed");
    let text = program.dump_text();

    assert!(
        text.contains("dynamic_import"),
        "static dynamic import should keep ModuleId fast path:\n{text}"
    );
    assert!(
        !text.contains("dynamic_import_runtime"),
        "static dynamic import should not use runtime expression path:\n{text}"
    );
}

#[test]
fn dynamic_import_static_literal_extra_arg_reports_unsupported_before_fast_path() {
    let mut dynamic_targets = std::collections::HashMap::new();
    dynamic_targets.insert(wjsm_ir::ModuleId(0), vec![wjsm_ir::ModuleId(1)]);
    let mut dynamic_specifiers = std::collections::HashMap::new();
    dynamic_specifiers.insert(
        wjsm_ir::ModuleId(0),
        vec![("./dep.js".to_string(), wjsm_ir::ModuleId(1))],
    );
    let mut export_names = std::collections::HashMap::new();
    export_names.insert(
        wjsm_ir::ModuleId(1),
        std::collections::BTreeSet::from(["value".to_string()]),
    );
    let error = lower_modules(
        vec![
            esm_input(
                0,
                "/project/main.js",
                "import('./dep.js', { with: { type: 'json' } });\n",
            ),
            esm_input(1, "/project/dep.js", "export const value = 1;\n"),
        ],
        &std::collections::HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>::new(),
        &dynamic_targets,
        &export_names,
        &dynamic_specifiers,
        &std::collections::HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ReExportBinding>>::new(),
    )
    .expect_err("extra import() options must be rejected before the static fast path");

    assert_unsupported_dynamic_import_extra_arg(error);
}

#[test]
fn import_meta_resolve_method_lowers_to_host_callable() {
    let text = lower_single_esm_source("console.log(import.meta.resolve('./dep.js'));\n");

    assert!(
        text.contains("import_meta.resolve"),
        "import.meta.resolve should be installed on import.meta:\n{text}"
    );
}

#[test]
fn fn_without_arguments_ref_elides_arguments_object() {
    // The whole point of the optimization: a plain `function inc(x){return x+1;}`
    // builds no arguments object and is therefore no-GC.
    let text = dump("function inc(x) { return x + 1; }\ninc(1);\n");
    assert!(
        !text.contains(ARGS_OBJ_MARKER),
        "function not referencing `arguments` must not materialise the arguments object:\n{text}"
    );
}

#[test]
fn fn_with_arguments_ref_keeps_arguments_object() {
    // When the body reads `arguments`, the object must still be built.
    let text = dump("function f() { return arguments.length; }\nf(1, 2);\n");
    assert!(
        text.contains(ARGS_OBJ_MARKER),
        "function referencing `arguments` must still materialise the arguments object:\n{text}"
    );
}

#[test]
fn arrow_referencing_arguments_keeps_enclosing_object() {
    // A nested arrow inherits the enclosing non-arrow function's `arguments`, so the
    // enclosing function must build it even though the reference is lexically inside
    // the arrow.
    let text = dump("function f() { return (() => arguments[0])(); }\nf(42);\n");
    assert!(
        text.contains(ARGS_OBJ_MARKER),
        "arrow referencing `arguments` must force the enclosing function to build it:\n{text}"
    );
}

#[test]
fn nested_fn_arguments_does_not_force_outer() {
    // `g` references its OWN `arguments`; `f` does not reference any. Only `g` should
    // build an arguments object — exactly one marker in the whole module.
    let text = dump(
        r#"
function f() {
  function g() { return arguments.length; }
  return g;
}
f();
"#,
    );
    let count = text.matches(ARGS_OBJ_MARKER).count();
    assert_eq!(
        count, 1,
        "only the inner `g` (which references `arguments`) should build the object, \
         got {count} occurrences:\n{text}"
    );
}

#[test]
fn eval_in_body_keeps_arguments_object() {
    // Direct `eval` could read `arguments` dynamically, so we conservatively keep it.
    let text = dump("function f() { eval(\"0\"); }\nf();\n");
    assert!(
        text.contains(ARGS_OBJ_MARKER),
        "direct eval in body must conservatively keep the arguments object:\n{text}"
    );
}
