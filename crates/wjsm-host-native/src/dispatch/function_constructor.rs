//! `Function` 动态函数构造器（§20.2.1.1，CreateDynamicFunction）的宿主实现。
//!
//! `Function(...)` 与 `new Function(...)` 语义一致：最后一个实参为函数体，
//! 其余实参逐个 ToString 后以 `,` 拼接为形参串；语法校验（含注入防护与
//! 严格模式早错误）由 `wjsm_semantic::prepare_dynamic_function` 承担；编译
//! 复用 eval 管线——匿名函数表达式脚本的完成值即目标闭包，其 ScopeRecord
//! 外层是全局对象（非词法捕获），与规范"[[Environment]] 为 realm 的
//! GlobalEnv"一致。

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::errors::javascript_error;
use super::runtime::fail_dispatch;
use super::{eval, modules, node_vm, to_string_coerced};
use crate::{FUNCTION_METADATA_FLAGS, NativeAgentState};

/// `Function` 构造器入口：调用形式与构造形式共用。
pub(crate) fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    arguments: &[i64],
) -> i64 {
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
    // §20.2.1.1.1 步骤 8/9：每个实参 ToString（Symbol → TypeError，
    // 用户 toString 抛出的异常原样传播）。
    let mut texts = Vec::with_capacity(arguments.len());
    for argument in arguments {
        match to_string_coerced(ctx, state, *argument) {
            Ok(text) => texts.push(text),
            Err(exception) => return exception,
        }
    }
    let (body, parameters) = match texts.split_last() {
        Some((body, parameters)) => (body.as_str(), parameters.join(",")),
        None => ("", String::new()),
    };
    let prepared = match wjsm_semantic::prepare_dynamic_function(&parameters, body) {
        Ok(prepared) => prepared,
        Err(message) => return javascript_error(ctx, state, "SyntaxError", message),
    };
    let logical_url = node_vm::next_url(state, "function-constructor");
    let result =
        modules::execute_vm_script(ctx, state, &prepared.compile_source, global, &logical_url);
    let function = eval::eval_execution_result(ctx, state, result);
    if value::is_exception(function) {
        return function;
    }
    if !value::is_callable(function) {
        return fail_dispatch(ctx);
    }
    apply_function_metadata(state, function, prepared.expected_length)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

/// §20.2.1.1.1 步骤 31/32：`length` 为 ExpectedArgumentCount，
/// `name` 为 `"anonymous"`（均 configurable、非 writable、非 enumerable）。
fn apply_function_metadata(
    state: &mut NativeAgentState,
    function: i64,
    expected_length: u32,
) -> Option<i64> {
    let callable = value::strip_gc_color(function);
    let name_key = state.intern_property_string("name".into())?;
    let anonymous = state.intern_text("anonymous".into(), value::TAG_STRING)?;
    state
        .callable_properties
        .insert((callable, name_key), anonymous);
    state
        .callable_property_flags
        .insert((callable, name_key), FUNCTION_METADATA_FLAGS);
    let length_key = state.intern_property_string("length".into())?;
    state
        .callable_properties
        .insert((callable, length_key), value::encode_f64(f64::from(expected_length)));
    state
        .callable_property_flags
        .insert((callable, length_key), FUNCTION_METADATA_FLAGS);
    Some(function)
}
