//! eval 桥接 builtin 的宿主实现：直接/间接 eval、作用域绑定读写。

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::{NativeRuntimeOp, NativeVmContext};

use super::errors::javascript_error;
use super::modules;
use super::node_vm;
use super::runtime::{self, fail_dispatch};
use crate::NativeAgentState;

pub(super) fn dispatch_eval(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::EvalIndirect => eval_indirect(ctx, state, args),
        Builtin::Eval => eval_dynamic(ctx, state, args),
        Builtin::EvalGetBinding => eval_get_binding(ctx, state, args),
        Builtin::EvalSetBinding => eval_set_binding(ctx, state, args),
        Builtin::EvalHasBinding => {
            let [environment, key] = args else {
                return Some(fail_dispatch(ctx));
            };
            value::encode_bool(eval_binding_exists(ctx, state, *environment, *key))
        }
        Builtin::EvalSuperBase => {
            let [environment] = args else {
                return Some(fail_dispatch(ctx));
            };
            modules::scope_record_super_base(state, *environment)
                .unwrap_or_else(value::encode_undefined)
        }
        _ => return None,
    })
}

/// 间接 eval：源码在新建的全局作用域记录中执行。
fn eval_indirect(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [code] = args else {
        return fail_dispatch(ctx);
    };
    let global = node_vm::current_context(state);
    if !node_vm::strings_enabled(state, global) {
        return modules::named_error_object(
            state,
            "EvalError",
            "Code generation from strings disallowed for this context".into(),
        )
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx));
    }
    let Some(source) = state.string_to_utf8(*code) else {
        return *code;
    };
    let Some(environment) = modules::create_scope_record_with_outer(state, global) else {
        return fail_dispatch(ctx);
    };
    let result =
        modules::execute_eval_script(ctx, state, &source, environment, global, "eval:indirect");
    modules::destroy_scope_record(state, environment);
    eval_execution_result(ctx, state, result)
}

/// 直接 eval：源码在传入的环境记录中执行。
fn eval_dynamic(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [code, environment] = args else {
        return fail_dispatch(ctx);
    };
    let global = node_vm::current_context(state);
    if !node_vm::strings_enabled(state, global) {
        return modules::named_error_object(
            state,
            "EvalError",
            "Code generation from strings disallowed for this context".into(),
        )
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx));
    }
    let Some(source) = state.string_to_utf8(*code) else {
        return *code;
    };
    let result =
        modules::execute_eval_script(ctx, state, &source, *environment, global, "eval:dynamic");
    eval_execution_result(ctx, state, result)
}

fn eval_get_binding(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [environment, key] = args else {
        return fail_dispatch(ctx);
    };
    if state.text_matches(*key, "__wjsm_new_target")
        && let Some(new_target) = modules::scope_record_new_target(state, *environment)
    {
        return new_target;
    }
    match modules::scope_record_get(state, *environment, *key) {
        modules::ScopeBindingRead::Value(result) => return result,
        modules::ScopeBindingRead::Uninitialized => {
            let name = eval_binding_name(state, *key);
            return javascript_error(
                ctx,
                state,
                "ReferenceError",
                format!("Cannot access '{name}' before initialization"),
            );
        }
        modules::ScopeBindingRead::Missing => {}
    }
    let outer = modules::scope_record_outer(state, *environment).unwrap_or(*environment);
    let Ok(result) = runtime::get_property(ctx, state, outer, *key) else {
        return fail_dispatch(ctx);
    };
    if !value::is_undefined(result) || eval_binding_exists(ctx, state, *environment, *key) {
        return result;
    }
    javascript_error(
        ctx,
        state,
        "ReferenceError",
        format!("{} is not defined", eval_binding_name(state, *key)),
    )
}

fn eval_set_binding(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [environment, key, stored] = args else {
        return fail_dispatch(ctx);
    };
    match modules::scope_record_set(state, *environment, *key, *stored) {
        modules::ScopeBindingWrite::Updated => return *stored,
        modules::ScopeBindingWrite::Constant => {
            return javascript_error(
                ctx,
                state,
                "TypeError",
                format!(
                    "assignment to constant `{}`",
                    eval_binding_name(state, *key)
                ),
            );
        }
        modules::ScopeBindingWrite::Missing => {}
    }
    if modules::scope_record_is_strict(state, *environment) {
        return javascript_error(
            ctx,
            state,
            "ReferenceError",
            format!(
                "assignment to undeclared variable `{}`",
                eval_binding_name(state, *key)
            ),
        );
    }
    let outer = modules::scope_record_outer(state, *environment).unwrap_or(*environment);
    runtime::dispatch_runtime(
        ctx,
        state,
        NativeRuntimeOp::SetProp,
        &[outer, *key, *stored],
        None,
    )
}

fn eval_binding_exists(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    environment: i64,
    key: i64,
) -> bool {
    if state.text_matches(key, "__wjsm_new_target")
        && modules::scope_record_new_target(state, environment).is_some()
    {
        return true;
    }
    if modules::scope_record_contains(state, environment, key) {
        return true;
    }
    let outer = modules::scope_record_outer(state, environment).unwrap_or(environment);
    runtime::get_property(ctx, state, outer, key).is_ok_and(|property| {
        !value::is_undefined(property) || runtime::has_property(state, outer, key)
    })
}

fn eval_binding_name(state: &NativeAgentState, key: i64) -> String {
    state
        .string_owned(key)
        .and_then(|text| text.to_utf8())
        .unwrap_or_else(|| runtime::render_value(state, key))
}

fn eval_execution_result(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    result: Result<i64, modules::VmExecutionError>,
) -> i64 {
    match result {
        Ok(result) => result,
        Err(modules::VmExecutionError::JavaScript(exception)) => exception,
        Err(modules::VmExecutionError::Compile(error)) => {
            if let Some(wjsm_semantic::LoweringError::Diagnostic(diagnostic)) =
                error.downcast_ref::<wjsm_semantic::LoweringError>()
            {
                if diagnostic.message.contains("cannot redeclare identifier") {
                    let identifier = diagnostic.message.split('`').nth(1).unwrap_or("<unknown>");
                    return javascript_error(
                        ctx,
                        state,
                        "SyntaxError",
                        format!("cannot redeclare identifier `{identifier}` in eval"),
                    );
                }
                if diagnostic
                    .message
                    .contains("cannot reassign a const-declared variable")
                {
                    let identifier = diagnostic.message.split('`').nth(1).unwrap_or("<unknown>");
                    return javascript_error(
                        ctx,
                        state,
                        "TypeError",
                        format!("assignment to constant `{identifier}`"),
                    );
                }
            }
            javascript_error(ctx, state, "SyntaxError", error.to_string())
        }
        Err(error) => javascript_error(ctx, state, "Error", error.to_string()),
    }
}
