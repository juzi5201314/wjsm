//! 从 JS 值读取 locale 列表与选项（走规范 Get，尊重原型污染）。

use wjsm_builtins::intl::{canonicalize_locale_list, lookup_supported_locales};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::IntlSlot;
use super::common::{intern, is_type_object, throw_intl};
use crate::NativeAgentState;
use crate::dispatch::runtime::{
    fail_dispatch, get_property, has_property, range_error, to_number_coerced, to_string_coerced,
    type_error,
};

pub(super) fn canonicalize_locales(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    locales: i64,
) -> Result<Vec<String>, i64> {
    if value::is_undefined(locales) {
        return Ok(Vec::new());
    }
    if value::is_string(locales) || is_locale_object(state, locales) {
        let tag = locale_tag(ctx, state, locales)?;
        return canonicalize_locale_list(&[tag]).map_err(|error| throw_intl(ctx, state, error));
    }
    let locales = to_object(ctx, state, locales)?;
    let length = length_of_array_like(ctx, state, locales)?;
    let mut tags = Vec::new();
    for index in 0..length {
        let key = intern(ctx, state, index.to_string());
        if value::is_exception(key) {
            return Err(key);
        }
        let present = has_property_js(ctx, state, locales, key)?;
        if !present {
            continue;
        }
        let item = match get_property(ctx, state, locales, key) {
            Ok(item) => item,
            Err(()) => return Err(fail_dispatch(ctx)),
        };
        if value::is_exception(item) {
            return Err(item);
        }
        tags.push(locale_tag(ctx, state, item)?);
    }
    canonicalize_locale_list(&tags).map_err(|error| throw_intl(ctx, state, error))
}

pub(super) fn supported_locales_of(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let locales = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let requested = match canonicalize_locales(ctx, state, locales) {
        Ok(requested) => requested,
        Err(exception) => return exception,
    };
    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    // SupportedLocales：options 非 undefined 时 ToObject，以便读取原型上的 localeMatcher。
    let options = if value::is_undefined(options) {
        match get_options_object(ctx, state, options) {
            Ok(options) => options,
            Err(exception) => return exception,
        }
    } else {
        match to_object(ctx, state, options) {
            Ok(options) => options,
            Err(exception) => return exception,
        }
    };
    if let Err(exception) = get_option_string(
        ctx,
        state,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    ) {
        return exception;
    }
    match lookup_supported_locales(&requested) {
        Ok(supported) => string_array(ctx, state, &supported),
        Err(error) => throw_intl(ctx, state, error),
    }
}

pub(super) fn get_options_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
) -> Result<i64, i64> {
    if value::is_undefined(options) {
        return state
            .allocate_object_with_prototype(0, false, wjsm_gc::PROTO_NULL_SENTINEL)
            .map_err(|_| fail_dispatch(ctx));
    }
    if is_type_object(options) {
        return Ok(options);
    }
    Err(type_error(ctx, state, "options must be an object"))
}

pub(super) fn get_option_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
    name: &str,
    values: &[&str],
    fallback: Option<&str>,
) -> Result<Option<String>, i64> {
    let value = get_named(ctx, state, options, name)?;
    if value::is_undefined(value) {
        return Ok(fallback.map(str::to_owned));
    }
    let text = to_string_coerced(ctx, state, value)?;
    if !values.is_empty() && !values.contains(&text.as_str()) {
        return Err(range_error(
            ctx,
            state,
            &format!("invalid {name} option: {text}"),
        ));
    }
    Ok(Some(text))
}

pub(super) fn get_option_bool_opt(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
    name: &str,
) -> Result<Option<bool>, i64> {
    let value = get_named(ctx, state, options, name)?;
    if value::is_undefined(value) {
        return Ok(None);
    }
    Ok(Some(to_boolean(state, value)))
}

pub(super) fn get_number_option(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
    name: &str,
    min: f64,
    max: f64,
    fallback: Option<f64>,
) -> Result<Option<f64>, i64> {
    let value = get_named(ctx, state, options, name)?;
    if value::is_undefined(value) {
        return Ok(fallback);
    }
    let number = to_number_coerced(ctx, state, value)?;
    if !number.is_finite() || number < min || number > max {
        return Err(range_error(ctx, state, &format!("{name} is out of range")));
    }
    Ok(Some(number.floor()))
}

pub(super) fn get_named(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
) -> Result<i64, i64> {
    let key = intern(ctx, state, name);
    if value::is_exception(key) {
        return Err(key);
    }
    match get_property(ctx, state, object, key) {
        Ok(value) if value::is_exception(value) => Err(value),
        Ok(value) => Ok(value),
        Err(()) => Err(fail_dispatch(ctx)),
    }
}

pub(super) fn string_array(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    items: &[String],
) -> i64 {
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        let stored = intern(ctx, state, item.clone());
        if value::is_exception(stored) {
            return stored;
        }
        values.push(stored);
    }
    state
        .allocate_array_values(&values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

pub(super) fn locale_tag(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    value: i64,
) -> Result<String, i64> {
    if is_locale_object(state, value)
        && let Some(IntlSlot::Locale(slot)) = state.intl.slots.get(&value::decode_handle(value))
    {
        return Ok(slot.tag.clone());
    }
    if !value::is_string(value) && !is_type_object(value) {
        return Err(type_error(ctx, state, "locale must be a string or object"));
    }
    to_string_coerced(ctx, state, value)
}

pub(super) fn is_locale_object(state: &NativeAgentState, value: i64) -> bool {
    value::is_js_object(value)
        && matches!(
            state.intl.slots.get(&value::decode_handle(value)),
            Some(IntlSlot::Locale(_))
        )
}

fn has_property_js(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
) -> Result<bool, i64> {
    if value::is_proxy(object) {
        let result = crate::dispatch::proxy::has(ctx, state, &[object, key]);
        if value::is_exception(result) {
            return Err(result);
        }
        return Ok(crate::dispatch::runtime::is_truthy(state, result));
    }
    Ok(has_property(state, object, key))
}

pub(super) fn to_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    value: i64,
) -> Result<i64, i64> {
    if is_type_object(value) {
        return Ok(value);
    }
    if value::is_null(value) || value::is_undefined(value) {
        return Err(type_error(
            ctx,
            state,
            "Cannot convert undefined or null to object",
        ));
    }
    let prototype = if value::is_f64(value) {
        super::ensure_number_prototype(state)
            .map(value::decode_handle)
            .ok_or_else(|| fail_dispatch(ctx))?
    } else if value::is_symbol(value) {
        state
            .symbol_prototype
            .or(state.object_prototype)
            .map(value::decode_handle)
            .ok_or_else(|| fail_dispatch(ctx))?
    } else {
        state
            .object_prototype
            .map(value::decode_handle)
            .ok_or_else(|| fail_dispatch(ctx))?
    };
    let object = state
        .allocate_object_with_prototype(0, false, prototype)
        .map_err(|_| fail_dispatch(ctx))?;
    state
        .boxed_primitives
        .insert(value::decode_handle(object), value);
    Ok(object)
}

fn length_of_array_like(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
) -> Result<u32, i64> {
    let length = get_named(ctx, state, object, "length")?;
    let number = to_number_coerced(ctx, state, length)?;
    if !number.is_finite() || number <= 0.0 {
        return Ok(0);
    }
    Ok(number.min((1u64 << 32) as f64 - 1.0).floor() as u32)
}

pub(super) fn is_unicode_type(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|part| {
            (3..=8).contains(&part.len()) && part.chars().all(|ch| ch.is_ascii_alphanumeric())
        })
}

pub(super) fn require_unicode_type(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
    name: &str,
) -> Result<Option<String>, i64> {
    let value = get_option_string(ctx, state, options, name, &[], None)?;
    if let Some(text) = &value
        && !is_unicode_type(text)
    {
        return Err(range_error(ctx, state, &format!("invalid {name}")));
    }
    Ok(value)
}

pub(super) fn to_boolean(state: &NativeAgentState, encoded: i64) -> bool {
    if value::is_bool(encoded) {
        return value::decode_bool(encoded);
    }
    if value::is_undefined(encoded) || value::is_null(encoded) {
        return false;
    }
    if value::is_f64(encoded) {
        let number = value::decode_f64(encoded);
        return number != 0.0 && !number.is_nan();
    }
    if value::is_string(encoded) {
        return state.string(encoded).is_some_and(|text| !text.is_empty());
    }
    true
}
