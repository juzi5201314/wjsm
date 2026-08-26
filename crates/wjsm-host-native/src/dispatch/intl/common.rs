//! Intl 宿主共用：实例分配、错误与 resolvedOptions 对象。

use wjsm_builtins::intl::{IntlError, IntlErrorKind};
use wjsm_gc::HeapAccessV2Error;
use wjsm_intl_data::FormatPart;
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::install::ensure_constructor_prototype;
use super::{IntlCallable, IntlSlot};
use crate::dispatch::runtime::{fail_dispatch, range_error, type_error};
use crate::{ASSIGNED_PROPERTY_FLAGS, NativeAgentState, NativeCallableKind};

pub(super) fn intern(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    text: impl Into<String>,
) -> i64 {
    super::intern(ctx, state, text)
}

pub(super) fn is_type_object(encoded: i64) -> bool {
    !value::is_undefined(encoded)
        && !value::is_null(encoded)
        && !value::is_f64(encoded)
        && !value::is_bool(encoded)
        && !value::is_string(encoded)
        && !value::is_symbol(encoded)
        && !value::is_bigint(encoded)
}

pub(super) fn throw_intl(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    error: IntlError,
) -> i64 {
    match error.kind {
        IntlErrorKind::Type => type_error(ctx, state, &error.message),
        IntlErrorKind::Range => range_error(ctx, state, &error.message),
    }
}

pub(super) fn require_new(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
) -> Result<(), i64> {
    if value::is_undefined(current_new_target(state)) {
        Err(type_error(
            ctx,
            state,
            "Intl constructor must be called with new",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn current_new_target(state: &NativeAgentState) -> i64 {
    state
        .activations
        .last()
        .map(|activation| activation.new_target)
        .unwrap_or_else(value::encode_undefined)
}

pub(super) fn create_instance(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    constructor: IntlCallable,
    slot: IntlSlot,
    this_value: i64,
) -> i64 {
    let Some(fallback) = state.native_callable(NativeCallableKind::Intl(constructor)) else {
        return fail_dispatch(ctx);
    };
    let recorded_new_target = current_new_target(state);
    let new_target = if value::is_undefined(recorded_new_target) {
        fallback
    } else {
        recorded_new_target
    };
    let Some(prototype) = instance_prototype(state, new_target, fallback, constructor) else {
        return fail_dispatch(ctx);
    };
    // `new` / `super()` 已分配 receiver；槽必须写在该对象上，否则子类拿到空实例。
    let object = if !value::is_undefined(recorded_new_target) && value::is_js_object(this_value) {
        if state
            .gc
            .heap()
            .set_prototype(
                value::decode_handle(this_value),
                value::decode_handle(prototype),
            )
            .is_err()
        {
            return fail_dispatch(ctx);
        }
        this_value
    } else {
        match allocate_intl_instance(ctx, state, value::decode_handle(prototype)) {
            Ok(object) => object,
            Err(exception) => return exception,
        }
    };
    state.intl.slots.insert(value::decode_handle(object), slot);
    object
}

fn allocate_intl_instance(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    prototype: u32,
) -> Result<i64, i64> {
    match state.allocate_object_with_prototype(0, false, prototype) {
        Ok(object) => Ok(object),
        Err(HeapAccessV2Error::HeapExhausted { .. }) => {
            state.collect_garbage(ctx).map_err(|_| fail_dispatch(ctx))?;
            state
                .allocate_object_with_prototype(0, false, prototype)
                .map_err(|_| fail_dispatch(ctx))
        }
        Err(_) => Err(fail_dispatch(ctx)),
    }
}

pub(super) fn slot_handle(receiver: i64) -> Option<u32> {
    value::is_js_object(receiver).then(|| value::decode_handle(receiver))
}

pub(super) fn set_data(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
    stored: i64,
) -> Result<(), i64> {
    super::install::install_data_property(state, object, name, stored, ASSIGNED_PROPERTY_FLAGS)
        .map_err(|()| fail_dispatch(ctx))
}

pub(super) fn resolved_strings(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    pairs: &[(&'static str, String)],
) -> i64 {
    let mut fields = Vec::with_capacity(pairs.len());
    for (name, text) in pairs {
        fields.push((*name, intern(ctx, state, text.clone())));
    }
    resolved_object(ctx, state, &fields)
}

pub(super) fn resolved_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    fields: &[(&str, i64)],
) -> i64 {
    let object = match state.allocate_object_with_gc_retry(ctx, fields.len() as u32, false) {
        Ok(object) => object,
        Err(_) => return fail_dispatch(ctx),
    };
    for (name, stored) in fields {
        if let Err(exception) = set_data(ctx, state, object, name, *stored) {
            return exception;
        }
    }
    object
}

pub(super) fn parts_array(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    parts: Vec<FormatPart>,
) -> i64 {
    let mut values = Vec::with_capacity(parts.len());
    for part in parts {
        let object = match state.allocate_object_with_gc_retry(ctx, 3, false) {
            Ok(object) => object,
            Err(_) => return fail_dispatch(ctx),
        };
        let type_name = intern(ctx, state, part.type_name);
        let stored = intern(ctx, state, part.value);
        if let Err(exception) = set_data(ctx, state, object, "type", type_name) {
            return exception;
        }
        if let Err(exception) = set_data(ctx, state, object, "value", stored) {
            return exception;
        }
        if let Some(source) = part.source {
            let source = intern(ctx, state, source);
            if let Err(exception) = set_data(ctx, state, object, "source", source) {
                return exception;
            }
        }
        if let Some(unit) = part.unit {
            let unit = intern(ctx, state, unit);
            if let Err(exception) = set_data(ctx, state, object, "unit", unit) {
                return exception;
            }
        }
        values.push(object);
    }
    state
        .allocate_array_values_with_gc_retry(ctx, &values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn instance_prototype(
    state: &mut NativeAgentState,
    new_target: i64,
    fallback: i64,
    constructor: IntlCallable,
) -> Option<i64> {
    let key = state.intern_property_string("prototype".into())?;
    if value::is_callable(new_target)
        && let Some(prototype) = state
            .callable_property(new_target, key)
            .filter(|value| is_type_object(*value))
    {
        return Some(prototype);
    }
    ensure_constructor_prototype(state, fallback, constructor)
}
