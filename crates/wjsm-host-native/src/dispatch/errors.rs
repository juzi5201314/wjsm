//! 错误构造器（Error 及 6 个子类）的宿主实现。

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::modules;
use super::runtime::{fail_dispatch, get_property, has_property, to_string_coerced};
use crate::NativeAgentState;

pub(super) fn dispatch_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    // CallBuiltin 是普通函数调用（ES §20.5.1.1：NewTarget 为 undefined，
    // 原型落回内在 %NativeError.prototype%）。此路径不压新激活帧，绝不能
    // 读取外层函数激活帧的 new.target——否则构造器体内 `TypeError("x")`
    // 会误继承外层类的 prototype。
    Some(error_constructor(
        ctx,
        state,
        builtin,
        value::encode_undefined(),
        value::encode_undefined(),
        args,
    ))
}

/// Error 及其子类构造器的公共实现（dispatch 与 `NativeCallableKind::Builtin` 构造路径共用）。
/// `new_target` 由调用方按其调用形态显式传入：构造路径传激活帧的
/// new.target，普通调用路径传 undefined。
pub(crate) fn error_constructor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    this_value: i64,
    new_target: i64,
    args: &[i64],
) -> i64 {
    let name = match builtin {
        Builtin::ErrorConstructor => "Error",
        Builtin::EvalErrorConstructor => "EvalError",
        Builtin::RangeErrorConstructor => "RangeError",
        Builtin::ReferenceErrorConstructor => "ReferenceError",
        Builtin::SyntaxErrorConstructor => "SyntaxError",
        Builtin::TypeErrorConstructor => "TypeError",
        Builtin::URIErrorConstructor => "URIError",
        _ => return fail_dispatch(ctx),
    };
    let message = match args.first().copied() {
        None => String::new(),
        Some(message) if value::is_undefined(message) => String::new(),
        Some(message) => match to_string_coerced(ctx, state, message) {
            Ok(message) => message,
            Err(exception) => return exception,
        },
    };
    let Some(intrinsic_prototype) = state.ensure_error_prototype(name) else {
        return fail_dispatch(ctx);
    };
    let error = if !value::is_undefined(new_target) && value::is_js_object(this_value) {
        modules::initialize_error_object(state, this_value, name, message)
    } else {
        modules::named_error_object(state, name, message)
    };
    let Some(error) = error else {
        return fail_dispatch(ctx);
    };
    if !value::is_undefined(new_target) {
        let Some(prototype_key) = state.intern_property_string("prototype".into()) else {
            return fail_dispatch(ctx);
        };
        let prototype = state
            .callable_property(new_target, prototype_key)
            .filter(|prototype| value::is_js_object(*prototype))
            .unwrap_or(intrinsic_prototype);
        if state
            .gc
            .heap()
            .set_prototype(value::decode_handle(error), value::decode_handle(prototype))
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    let Some(options) = args.get(1).copied() else {
        return error;
    };
    if !value::is_js_object(options) && !value::is_regexp(options) {
        return error;
    }
    let Some(cause_key) = state.intern_text("cause".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let has_cause = match has_property(ctx, state, options, cause_key) {
        Ok(present) => present,
        Err(exception) => return exception,
    };
    if has_cause {
        let cause =
            get_property(ctx, state, options, cause_key).unwrap_or_else(|()| fail_dispatch(ctx));
        if value::is_exception(cause) {
            return cause;
        }
        if modules::set_named_property(state, error, "cause", cause).is_err() {
            return fail_dispatch(ctx);
        }
    }
    error
}

/// 按名称构造原生错误对象并包装为异常值。
pub(crate) fn javascript_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    name: &str,
    message: String,
) -> i64 {
    modules::named_error_object(state, name, message)
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
