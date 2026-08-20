//! 运算符 / 反射类 builtin 的宿主实现：比较、typeof、instanceof、in、
//! 异常值桥、new.target、debugger 等零散语义。

use std::cmp::Ordering;

use num_bigint::BigInt;
use num_traits::FromPrimitive;

use wjsm_ir::{Builtin, value, wk_symbol};
use wjsm_native_abi::NativeVmContext;

use super::bigint;
use super::proxy;
use super::runtime::PrimitiveHint;
use super::runtime::{
    abstract_equal, fail_dispatch, get_property, has_property, is_truthy, strict_equal, to_number,
    to_primitive, type_error,
};
use crate::NativeAgentState;

pub(super) fn dispatch_operator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::AbstractCompare => abstract_compare_builtin(ctx, state, args),
        Builtin::AbstractEq => {
            let [left, right] = args else {
                return Some(fail_dispatch(ctx));
            };
            match abstract_equal(ctx, state, *left, *right) {
                Ok(equal) => value::encode_bool(equal),
                Err(exception) => exception,
            }
        }
        Builtin::StrictEq => {
            let [left, right] = args else {
                return Some(fail_dispatch(ctx));
            };
            value::encode_bool(strict_equal(state, *left, *right))
        }
        Builtin::TypeOf => type_of(ctx, state, args),
        Builtin::InstanceOf => instance_of(ctx, state, args),
        Builtin::In => {
            let [object, key] = args else {
                return Some(fail_dispatch(ctx));
            };
            if value::is_proxy(*object) {
                proxy::has(ctx, state, &[*object, *key])
            } else {
                value::encode_bool(has_property(state, *object, *key))
            }
        }
        Builtin::Throw => args
            .first()
            .and_then(|argument| state.create_exception(*argument))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::ExceptionValue => args
            .first()
            .and_then(|exception| state.exception_value(*exception))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::NewTarget => state
            .activations
            .last()
            .map(|activation| activation.new_target)
            .unwrap_or_else(value::encode_undefined),
        Builtin::Debugger => {
            crate::inspector::pause(ctx, state, "debuggerStatement");
            value::encode_undefined()
        }
        Builtin::IsCallable => args
            .first()
            .map(|input| value::encode_bool(value::is_callable(*input)))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::IsJsObject => args
            .first()
            .map(|input| value::encode_bool(value::is_js_object(*input)))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::GetPrototypeFromConstructor => {
            let Some(constructor) = args.first().copied() else {
                return Some(fail_dispatch(ctx));
            };
            let Some(prototype_key) = state.intern_property_string("prototype".into()) else {
                return Some(fail_dispatch(ctx));
            };
            state
                .callable_property(constructor, prototype_key)
                .filter(|prototype| value::is_js_object(*prototype))
                .unwrap_or_else(value::encode_null)
        }
        _ => return None,
    })
}

/// 抽象关系比较：`reverse` 交换左右操作数，`invert` 反转结果（`>` / `>=`）。
fn abstract_compare_builtin(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [left, right, reverse, invert] = args else {
        return fail_dispatch(ctx);
    };
    let left = match to_primitive(ctx, state, *left, PrimitiveHint::Number) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let right = match to_primitive(ctx, state, *right, PrimitiveHint::Number) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let comparison = if value::decode_bool(*reverse) {
        abstract_compare(state, right, left)
    } else {
        abstract_compare(state, left, right)
    };
    let result = if value::decode_bool(*invert) {
        comparison.is_some_and(|ordering| ordering != Ordering::Less)
    } else {
        comparison == Some(Ordering::Less)
    };
    value::encode_bool(result)
}

fn type_of(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(input) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let name = if value::is_undefined(input) {
        "undefined"
    } else if value::is_bool(input) {
        "boolean"
    } else if value::is_string(input) {
        "string"
    } else if value::is_callable(input) {
        "function"
    } else if value::is_bigint(input) {
        "bigint"
    } else if value::is_symbol(input) {
        "symbol"
    } else if value::is_null(input) || value::is_js_object(input) || value::is_regexp(input) {
        "object"
    } else {
        "number"
    };
    state
        .intern_text(name.into(), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn instance_of(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [object, constructor] = args else {
        return fail_dispatch(ctx);
    };
    let has_instance_key = value::encode_handle(value::TAG_SYMBOL, wk_symbol::HAS_INSTANCE);
    let method = match get_property(ctx, state, *constructor, has_instance_key) {
        Ok(method) => method,
        Err(()) => {
            return type_error(ctx, state, "Right-hand side of instanceof is not an object");
        }
    };
    if value::is_exception(method) {
        return method;
    }
    if !value::is_undefined(method) {
        if !value::is_callable(method) {
            return type_error(ctx, state, "Symbol.hasInstance method is not callable");
        }
        let result = state
            .invoke_callable(ctx, method, *constructor, &[*object])
            .unwrap_or_else(|| fail_dispatch(ctx));
        if value::is_exception(result) {
            return result;
        }
        return value::encode_bool(is_truthy(state, result));
    }
    if !state.is_callable_value(*constructor) {
        return type_error(ctx, state, "Right-hand side of instanceof is not callable");
    }
    let Some(prototype_key) = state.intern_text("prototype".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let prototype = match get_property(ctx, state, *constructor, prototype_key) {
        Ok(prototype) => prototype,
        Err(()) => return fail_dispatch(ctx),
    };
    if value::is_exception(prototype) {
        return prototype;
    }
    if !(value::is_object(prototype)
        || value::is_array(prototype)
        || value::is_callable(prototype)
        || value::is_proxy(prototype))
    {
        return type_error(
            ctx,
            state,
            "Function has non-object prototype in instanceof check",
        );
    }
    value::encode_bool(state.prototype_chain_contains_value(*object, prototype))
}

fn abstract_compare(state: &NativeAgentState, left: i64, right: i64) -> Option<Ordering> {
    if value::is_string(left) && value::is_string(right) {
        return state
            .string(left)?
            .as_flat_slice()
            .partial_cmp(state.string(right)?.as_flat_slice());
    }
    match (value::is_bigint(left), value::is_bigint(right)) {
        (true, true) => bigint_compare(state, left, right),
        (true, false) => {
            bigint_number_compare(&bigint::read(state, left)?, to_number(state, right)?)
        }
        (false, true) => {
            bigint_number_compare(&bigint::read(state, right)?, to_number(state, left)?)
                .map(Ordering::reverse)
        }
        (false, false) => to_number(state, left)?.partial_cmp(&to_number(state, right)?),
    }
}

fn bigint_compare(state: &NativeAgentState, left: i64, right: i64) -> Option<Ordering> {
    bigint::read(state, left)?.partial_cmp(&bigint::read(state, right)?)
}

fn bigint_number_compare(bigint: &BigInt, number: f64) -> Option<Ordering> {
    if number.is_nan() {
        return None;
    }
    if number == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if number == f64::NEG_INFINITY {
        return Some(Ordering::Greater);
    }
    let integral = BigInt::from_f64(number.trunc())?;
    let comparison = bigint.cmp(&integral);
    if comparison != Ordering::Equal || number.fract() == 0.0 {
        return Some(comparison);
    }
    Some(if number.is_sign_positive() {
        Ordering::Less
    } else {
        Ordering::Greater
    })
}
