//! WithStatement 对象环境记录的宿主原语：HasBinding 探测与头部 ToObject。

use wjsm_ir::{Builtin, value, wk_symbol};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{self, fail_dispatch, type_error};
use super::{intl, proxy};
use crate::NativeAgentState;

pub(super) fn dispatch_with(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::WithHasBinding => {
            let [object, key] = args else {
                return Some(fail_dispatch(ctx));
            };
            match with_has_binding(ctx, state, *object, *key) {
                Ok(has) => value::encode_bool(has),
                Err(exception) => exception,
            }
        }
        Builtin::WithToObject => {
            let [input] = args else {
                return Some(fail_dispatch(ctx));
            };
            with_to_object(ctx, state, *input)
        }
        _ => return None,
    })
}

/// §9.1.1.2.1 对象环境记录 HasBinding：`? HasProperty(bindings, N)` 后按
/// `@@unscopables` 过滤。Proxy has trap / unscopables getter 抛出的异常
/// 以 `Err(exception)` 原样传播。
pub(crate) fn with_has_binding(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
) -> Result<bool, i64> {
    if !has_property_abrupt(ctx, state, object, key)? {
        return Ok(false);
    }
    let unscopables_key = value::encode_handle(value::TAG_SYMBOL, wk_symbol::UNSCOPABLES);
    let unscopables = get_property_abrupt(ctx, state, object, unscopables_key)?;
    if !is_object_like(unscopables) {
        return Ok(true);
    }
    let blocked = get_property_abrupt(ctx, state, unscopables, key)?;
    Ok(!runtime::is_truthy(state, blocked))
}

/// with 头部的 ToObject（§7.1.18）：null/undefined 抛 TypeError；对象 /
/// callable / Proxy 原样返回；原语装箱为携带对应原型的包装对象。
fn with_to_object(ctx: &mut NativeVmContext, state: &mut NativeAgentState, input: i64) -> i64 {
    if value::is_null(input) || value::is_undefined(input) {
        return type_error(ctx, state, "Cannot convert undefined or null to object");
    }
    if is_object_like(input) {
        return input;
    }
    let prototype = if value::is_f64(input) {
        intl::ensure_number_prototype(state)
    } else if value::is_string(input) {
        intl::ensure_string_prototype(state)
    } else if value::is_bigint(input) {
        intl::ensure_bigint_prototype(state)
    } else if value::is_symbol(input) {
        state.symbol_prototype.or(state.object_prototype)
    } else {
        state.object_prototype
    };
    let Some(prototype) = prototype.map(value::decode_handle) else {
        return fail_dispatch(ctx);
    };
    let Ok(object) = state.allocate_object_with_prototype(0, false, prototype) else {
        return fail_dispatch(ctx);
    };
    state
        .boxed_primitives
        .insert(value::decode_handle(object), input);
    object
}

/// HasProperty（含原型链）：Proxy 走 has trap 并传播异常。
fn has_property_abrupt(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
) -> Result<bool, i64> {
    if value::is_proxy(object) {
        let result = proxy::has(ctx, state, &[object, key]);
        if value::is_exception(result) {
            return Err(result);
        }
        return Ok(runtime::is_truthy(state, result));
    }
    Ok(runtime::has_property(state, object, key))
}

/// [[Get]]：getter / proxy get trap 异常以 `Err` 传播。
fn get_property_abrupt(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
) -> Result<i64, i64> {
    let result = runtime::get_property(ctx, state, object, key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(result) {
        return Err(result);
    }
    Ok(result)
}

/// ECMAScript Type(value) 为 Object（含 callable / Proxy / 数组等宿主表示）。
fn is_object_like(input: i64) -> bool {
    !value::is_undefined(input)
        && !value::is_null(input)
        && !value::is_f64(input)
        && !value::is_bool(input)
        && !value::is_string(input)
        && !value::is_symbol(input)
        && !value::is_bigint(input)
}
