use std::path::{Path, PathBuf};

use wjsm_parser::parse_module;
use wjsm_semantic::{
    LoweringError, ModuleKind, ModuleLinking, ModuleLoweringInput, ModuleMetadata, lower_module,
    lower_modules,
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
fn tco_countdown_fixture_matches_ir_snapshot() {
    assert_snapshot("tco_countdown");
}

#[test]
fn async_compound_await_loop_fixture_matches_ir_snapshot() {
    assert_snapshot("async_compound_await_loop");
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
fn tdz_same_function_forward_reference_stays_compile_rejected() {
    // 同函数内的直线前向引用必然在运行时抛错，静态判定零开销且与文档承诺一致。
    let rejected = [
        (
            "const set = { value: set };",
            "cannot access `set` before initialisation",
        ),
        (
            "const set = { [set]: 1 };",
            "cannot access `set` before initialisation",
        ),
        (
            "const set = { ...set };",
            "cannot access `set` before initialisation",
        ),
        (
            "const set = console.log(set);",
            "cannot access `set` before initialisation",
        ),
        (
            "const set = { m() { return set; let set; } };",
            "cannot access `set` before initialisation",
        ),
        (
            "const set = { m() { set = set; } };",
            "cannot reassign a const-declared variable `set`",
        ),
        (
            "const set = { m() { set++; } };",
            "cannot reassign a const-declared variable `set`",
        ),
    ];

    for (source, expected_message) in rejected {
        let error = lower_module(parse_module(source).expect("parse should succeed"), false)
            .expect_err("same-function TDZ forward reference should reject this source");
        match error {
            LoweringError::Diagnostic(diagnostic) => {
                assert!(
                    diagnostic.message.contains(expected_message),
                    "source {source:?} produced unexpected diagnostic: {}",
                    diagnostic.message
                );
            }
        }
    }
}

#[test]
fn tdz_cross_function_forward_reference_lowers_with_runtime_check() {
    // 跨函数前向引用静态无法判定执行是否先于声明，降级为运行时 TdzCheck：
    // 声明执行前 env 槽持有未初始化哨兵，读取时抛 ReferenceError。
    let runtime_checked = [
        "let x = { m() { return x; } }.m();",
        "let x = { m() { return x; } }.m;",
        "let x = { get self() { return x; } }.self;",
        "let x = true ? { m() { return x; } } : {};",
        "let x = [{ m() { return x; } }];",
        "function use(value) { return value; } let x = use({ m() { return x; } });",
        "function Box(value) { return value; } let x = new Box({ m() { return x; } });",
        "const set = () => set;",
        "const set = function () { return set; };",
        "const o = { m() { return x; } }; let x = 1;",
        "const wrapped = ((({ m() { return wrapped; } } as const) as object) satisfies object)!;",
    ];

    for source in runtime_checked {
        let program = lower_module(parse_module(source).expect("parse should succeed"), false)
            .unwrap_or_else(|error| {
                panic!("cross-function TDZ forward reference should lower {source:?}: {error:?}")
            });
        let text = program.dump_text();
        assert!(
            text.contains("builtin.tdz_check"),
            "source {source:?} should emit a runtime TdzCheck, got:\n{text}"
        );
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
fn licm_shape_check_hoist_loop_fixture_matches_ir_snapshot() {
    assert_snapshot("licm_shape_check_hoist_loop");
}

#[test]
fn licm_shape_mutation_loop_fixture_matches_ir_snapshot() {
    assert_snapshot("licm_shape_mutation_loop");
}

#[test]
fn licm_elem_guard_hoist_loop_fixture_matches_ir_snapshot() {
    assert_snapshot("licm_elem_guard_hoist_loop");
}

#[test]
fn licm_elem_guard_mutation_loop_fixture_matches_ir_snapshot() {
    assert_snapshot("licm_elem_guard_mutation_loop");
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
fn call_spread_args_fixture_matches_ir_snapshot() {
    assert_snapshot("call_spread_args");
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
fn optional_chain_short_circuit_fixture_matches_ir_snapshot() {
    assert_snapshot("optional_chain_short_circuit");
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

#[test]
fn class_gen_method_capture_fixture_matches_ir_snapshot() {
    assert_snapshot("class_gen_method_capture");
}

#[test]
fn class_private_generator_method_fixture_matches_ir_snapshot() {
    assert_snapshot("class_private_generator_method");
}

#[test]
fn class_private_async_method_fixture_matches_ir_snapshot() {
    assert_snapshot("class_private_async_method");
}

#[test]
fn class_super_constructor_fixture_matches_ir_snapshot() {
    assert_snapshot("class_super_constructor");
}

#[test]
fn class_super_return_object_fixture_matches_ir_snapshot() {
    assert_snapshot("class_super_return_object");
}

#[test]
fn private_in_brand_check_fixture_matches_ir_snapshot() {
    assert_snapshot("private_in_brand_check");
}

#[test]
fn in_instanceof_exceptions_fixture_matches_ir_snapshot() {
    assert_snapshot("in_instanceof_exceptions");
}

#[test]
fn binary_operand_exceptions_fixture_matches_ir_snapshot() {
    assert_snapshot("binary_operand_exceptions");
}

#[test]
fn binary_operand_exceptions_compare_fixture_matches_ir_snapshot() {
    assert_snapshot("binary_operand_exceptions_compare");
}

#[test]
fn binary_operand_exceptions_async_fixture_matches_ir_snapshot() {
    assert_snapshot("binary_operand_exceptions_async");
}

#[test]
fn primitive_setprop_strict_fixture_matches_ir_snapshot() {
    assert_snapshot("primitive_setprop_strict");
}

#[test]
fn issue373_loop_closure_capture_fixture_matches_ir_snapshot() {
    assert_snapshot("issue373_loop_closure_capture");
}

#[test]
fn issue384_object_method_self_reference_fixture_matches_ir_snapshot() {
    assert_snapshot("issue384_object_method_self_reference");
}

#[test]
fn object_literal_static_keys_fixture_matches_ir_snapshot() {
    assert_snapshot("object_literal_static_keys");
}

#[test]
fn object_computed_key_eval_order_fixture_matches_ir_snapshot() {
    assert_snapshot("object_computed_key_eval_order");
}

#[test]
fn escape_scalar_record_loop_fixture_matches_ir_snapshot() {
    assert_snapshot("escape_scalar_record_loop");
}

#[test]
fn async_method_super_fixture_matches_ir_snapshot() {
    assert_snapshot("async_method_super");
}

#[test]
fn fn_expr_name_binding_fixture_matches_ir_snapshot() {
    assert_snapshot("fn_expr_name_binding");
}

// ── with 语句 ───────────────────────────────────────────────────────────

#[test]
fn with_basic_fixture_matches_ir_snapshot() {
    assert_snapshot("with_basic");
}

#[test]
fn with_eval_fixture_matches_ir_snapshot() {
    assert_snapshot("with_eval");
}

#[test]
fn with_in_strict_code_reports_diagnostic() {
    // §14.11.1 early error：模块级指令、函数级指令、类体均为严格代码。
    let rejected = [
        "\"use strict\"; with ({}) {}",
        "function f() { \"use strict\"; with ({}) {} }",
        "class C { m() { with ({}) {} } }",
        "class C { static { with ({}) {} } }",
        "const g = () => { \"use strict\"; with ({}) {} };",
    ];
    for source in rejected {
        let error = lower_module(parse_module(source).expect("parse should succeed"), false)
            .expect_err("strict code containing with should be rejected");
        match error {
            LoweringError::Diagnostic(diagnostic) => {
                assert!(
                    diagnostic
                        .message
                        .contains("Strict mode code may not include a with statement"),
                    "source {source:?} produced unexpected diagnostic: {}",
                    diagnostic.message
                );
            }
        }
    }
}

#[test]
fn with_in_sloppy_function_inside_strict_free_module_lowers() {
    // 非严格代码中的 with 正常降级为对象环境记录分派。
    let text = dump("const o = { x: 1 }; with (o) { console.log(x); }\n");
    assert!(
        text.contains("with.to_object") && text.contains("with.has_binding"),
        "with statement should lower to object environment dispatch:\n{text}"
    );
}

#[test]
fn with_dispatch_in_generator_loop_header_lowering_terminates() {
    // 回归：with 分派挂在生成器/async 循环头（回边目标）时，
    // inline_for_ea 的 reaching 数据流曾因非单调混沌迭代进入
    // Some/None 周期振荡而永不收敛（编译死循环）。
    let sources = [
        "function* g() { with ({ n: 2 }) { let i = 0; while (i < n) { yield i; i++; } } }\n",
        "async function a() { with ({ n: 2 }) { let i = 0; while (i < n) { await 0; i++; } } }\n",
    ];
    for source in sources {
        let program = lower_module(parse_module(source).expect("parse should succeed"), false)
            .unwrap_or_else(|error| panic!("lowering should terminate for {source:?}: {error:?}"));
        assert!(
            program.dump_text().contains("with.has_binding"),
            "generator/async with dispatch should survive lowering: {source:?}"
        );
    }
}

// ── 脚本模式全局环境记录（ES §9.1.1.4 / §16.1.7 GDI）─────────────────────
//
// 脚本模式（`lower_module(_, true)`）顶层声明经 GlobalDeclarationInstantiation
// 序幕进入全局环境记录：var/函数 → 对象记录（globalThis 属性），let/const/class
// → 声明式记录（宿主 GlobalEnvRecord）。命中名字的读/写全部路由 GlobalEnv 系列
// builtin，不再降级为 `$0.*` 槽。

#[test]
fn script_mode_gdi_prologue_routes_global_bindings() {
    let source = "var v = 1;\nlet l = 2;\nconst c = 3;\nfunction f() { return l; }\nclass K {}\nl = v + c;\n";
    let module = parse_module(source).expect("parse should succeed");
    let program = lower_module(module, true).expect("script lowering should succeed");
    let text = program.dump_text();

    for marker in [
        // 冲突预检 + 声明：词法名 DeclareLex、var 名 DeclareVar、函数名 DeclareFunc。
        "global_env.check",
        "global_env.declare_lex",
        "global_env.declare_var",
        "global_env.declare_func",
        // 词法初始化（解除 TDZ）与读改写路由。
        "global_env.init_lex",
        "global_env.get",
        "global_env.set",
    ] {
        assert!(
            text.contains(marker),
            "missing {marker} in script IR:\n{text}"
        );
    }
    // 脚本全局绑定不再落 `$0.*` 槽（函数声明与 var 均由宿主记录承载）。
    for absent in [
        "store_var $0.v",
        "store_var $0.l",
        "store_var $0.c",
        "store_var $0.f",
    ] {
        assert!(
            !text.contains(absent),
            "script global should not use IR slot {absent}:\n{text}"
        );
    }
}

#[test]
fn module_mode_does_not_emit_global_env_builtins() {
    let source = "var v = 1;\nlet l = 2;\nconsole.log(v + l);\n";
    let module = parse_module(source).expect("parse should succeed");
    let program = lower_module(module, false).expect("module lowering should succeed");
    let text = program.dump_text();
    assert!(
        !text.contains("global_env."),
        "module mode must keep `$0.*` slot bindings:\n{text}"
    );
}

#[test]
fn dataview_bigint_methods_lower_to_call_builtin() {
    // 静态已知 DataView 绑定的 getBigInt64/getBigUint64/setBigInt64/setBigUint64
    // 直连专用 CallBuiltin（与其余 get/set 族一致），不走通用属性调用。
    let source = "const view = new DataView(new ArrayBuffer(16));\nview.setBigInt64(0, 1n);\nview.setBigUint64(8, 1n, true);\nconsole.log(view.getBigInt64(0), view.getBigUint64(8, true));\n";
    let module = parse_module(source).expect("parse should succeed");
    let program = lower_module(module, false).expect("lowering should succeed");
    let text = program.dump_text();
    for marker in [
        "DataView.prototype.getBigInt64",
        "DataView.prototype.getBigUint64",
        "DataView.prototype.setBigInt64",
        "DataView.prototype.setBigUint64",
    ] {
        assert!(
            text.contains(marker),
            "expected `{marker}` CallBuiltin in IR:\n{text}"
        );
    }
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
        ModuleLinking::empty(),
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
        ModuleLinking::empty(),
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
        ModuleLinking::empty(),
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

// ImportCall（ES §13.3.10.1）步骤 2-3 为 `? Evaluation` / `? GetValue`：
// specifier 求值抛出必须同步传播（分叉到 exception_value + throw），
// 不得转为返回 promise 的 rejection；仅 ToString(specifier) 起才由宿主
// IfAbruptRejectPromise。以下用例断言同步抛出分叉且 runtime 调用只有一处。

#[test]
fn dynamic_import_json_parse_abrupt_lowers_to_synchronous_throw() {
    let text = lower_single_esm_source("import(JSON.parse('bad'));\n");

    assert!(
        text.contains("dynamic_import_runtime"),
        "runtime dynamic import path should exist for the normal completion:\n{text}"
    );
    assert_eq!(
        text.matches("dynamic_import_runtime").count(),
        1,
        "specifier abrupt must propagate synchronously, not fork a second runtime call:\n{text}"
    );
    let exception_value_pos = text
        .find("exception_value")
        .expect("specifier abrupt fork should unwrap the exception for a synchronous throw");
    let runtime_pos = text.find("dynamic_import_runtime").unwrap();
    assert!(
        exception_value_pos < runtime_pos,
        "specifier abrupt must be unwrapped and thrown before the runtime call:\n{text}"
    );
}

#[test]
fn dynamic_import_import_meta_resolve_abrupt_lowers_to_synchronous_throw() {
    let text = lower_single_esm_source("import(import.meta.resolve('./missing.js'));\n");

    assert!(
        text.contains("import_meta.resolve") && text.contains("dynamic_import_runtime"),
        "import.meta.resolve specifier should keep the runtime dynamic import path:\n{text}"
    );
    assert_eq!(
        text.matches("dynamic_import_runtime").count(),
        1,
        "import.meta.resolve abrupt must propagate synchronously, not fork a second runtime call:\n{text}"
    );
    assert!(
        text.contains("exception_value"),
        "import.meta.resolve abrupt fork should unwrap the exception for a synchronous throw:\n{text}"
    );
}

#[test]
fn dynamic_import_composed_json_parse_abrupt_lowers_to_synchronous_throw() {
    let text = lower_single_esm_source("import(JSON.parse('bad') + './never.js');\n");

    assert!(
        text.contains("JSON.parse") && text.contains("dynamic_import_runtime"),
        "composed specifier should keep the runtime dynamic import path:\n{text}"
    );
    assert_eq!(
        text.matches("dynamic_import_runtime").count(),
        1,
        "composed specifier abrupt must propagate synchronously before stringification:\n{text}"
    );
}

#[test]
fn dynamic_import_conditional_json_parse_abrupt_lowers_to_synchronous_throw() {
    let text = lower_single_esm_source("import((true ? JSON.parse('bad') : './dep.js') + '?x');\n");

    assert!(
        text.contains("JSON.parse") && text.contains("dynamic_import_runtime"),
        "conditional specifier should keep the runtime dynamic import path:\n{text}"
    );
    assert_eq!(
        text.matches("dynamic_import_runtime").count(),
        1,
        "conditional specifier abrupt must propagate synchronously before stringification:\n{text}"
    );
}

#[test]
fn dynamic_import_sequence_json_parse_abrupt_lowers_to_synchronous_throw() {
    let text = lower_single_esm_source("import((JSON.parse('bad'), './dep.js'));\n");

    assert!(
        text.contains("JSON.parse") && text.contains("dynamic_import_runtime"),
        "sequence specifier should keep the runtime dynamic import path:\n{text}"
    );
    assert_eq!(
        text.matches("dynamic_import_runtime").count(),
        1,
        "sequence abrupt must propagate synchronously; the final specifier must not mask it:\n{text}"
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
fn dynamic_import_composed_import_meta_resolve_abrupt_lowers_to_synchronous_throw() {
    let text = lower_single_esm_source("import(import.meta.resolve('./missing.js') + '?x');\n");

    assert!(
        text.contains("import_meta.resolve") && text.contains("dynamic_import_runtime"),
        "composed import.meta.resolve specifier should keep the runtime dynamic import path:\n{text}"
    );
    assert_eq!(
        text.matches("dynamic_import_runtime").count(),
        1,
        "composed import.meta.resolve abrupt must propagate synchronously before stringification:\n{text}"
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
        ModuleLinking {
            dynamic_import_targets: &dynamic_targets,
            export_names: &export_names,
            dynamic_import_specifiers: &dynamic_specifiers,
            ..ModuleLinking::empty()
        },
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
        ModuleLinking {
            dynamic_import_targets: &dynamic_targets,
            export_names: &export_names,
            dynamic_import_specifiers: &dynamic_specifiers,
            ..ModuleLinking::empty()
        },
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

#[test]
fn proven_number_loop_drops_is_exception_and_abstract_compare() {
    let text = dump(
        "let sum = 0;\nfor (let i = 1; i <= 3; i = i + 1) {\n  sum = sum + i;\n}\nconsole.log(sum);\n",
    );
    assert!(
        !text.contains("abstract_compare"),
        "Number relational compare should be IR Compare:\n{text}"
    );
    assert!(
        text.contains(" = lteq ") || text.contains(" = lt ") || text.contains(" = gteq "),
        "expected relational Compare:\n{text}"
    );
    for line in text.lines() {
        if line.contains("is_exception") && !line.contains("console") {
            let value = line.split_whitespace().last().unwrap_or("");
            let produced_by_add = text.lines().any(|def| {
                def.contains(&format!("{value} = add"))
                    || def.contains(&format!("{value} = sub"))
                    || def.contains(&format!("{value} = mul"))
            });
            assert!(
                !produced_by_add,
                "Number arithmetic must not keep is_exception:\n{text}"
            );
        }
    }
}

#[test]
fn captured_sroa_slots_are_not_frame_local() {
    let source =
        std::fs::read_to_string(workspace_root().join("fixtures/happy/timer_zero_delay_chain.js"))
            .expect("timer fixture");
    let lowered = lower_module(parse_module(&source).expect("parse"), false).expect("lower");
    for function in lowered.functions() {
        let locals = lowered.frame_local_variable_names(function);
        let leaked: Vec<_> = locals
            .iter()
            .copied()
            .filter(|name| name.starts_with("$sroa."))
            .collect();
        assert!(
            leaked.is_empty(),
            "{} must not promote shared $sroa slots: {leaked:?}",
            function.name()
        );
    }
}

#[test]
fn inlined_iife_clears_dead_object_slots() {
    let source = std::fs::read_to_string(
        workspace_root().join("fixtures/happy/finalization_registry_cleanup.js"),
    )
    .expect("finalization fixture");
    let text = dump(&source);
    let main = text.split("fn @$module_main").nth(1).expect("module main");
    let stores = main.matches("store var $2.target").count();
    assert!(
        stores >= 3,
        "inlined IIFE must write back undefined to $2.target on return so gc() can collect:\n{main}"
    );
}
