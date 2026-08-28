//! ECMA-262 locale 敏感方法，委托 Phase 2 的 Intl owner。

use wjsm_intl_data::{NormalizationForm, case_map, locale_case_map, normalize};
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::common::intern;
use super::js::canonicalize_locales;
use super::{IntlCallable, collator, datetime_format, number_format};
use crate::NativeAgentState;
use crate::NativeCallableKind;
use crate::dispatch::runtime::{
    fail_dispatch, get_property, has_property, to_number_coerced, to_string_coerced, type_error,
};

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: IntlCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    match callable {
        IntlCallable::StringNormalize => string_normalize(ctx, state, receiver, args),
        IntlCallable::StringToLowerCase => string_case(ctx, state, receiver, false, None),
        IntlCallable::StringToUpperCase => string_case(ctx, state, receiver, true, None),
        IntlCallable::StringToLocaleLowerCase => {
            string_locale_case(ctx, state, receiver, args, false)
        }
        IntlCallable::StringToLocaleUpperCase => {
            string_locale_case(ctx, state, receiver, args, true)
        }
        IntlCallable::StringLocaleCompare => locale_compare(ctx, state, receiver, args),
        IntlCallable::NumberToLocaleString => number_to_locale(ctx, state, receiver, args, false),
        IntlCallable::BigIntToLocaleString => number_to_locale(ctx, state, receiver, args, true),
        IntlCallable::ArrayToLocaleString => array_to_locale(ctx, state, receiver, args),
        _ => fail_dispatch(ctx),
    }
}

fn require_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
) -> Result<String, i64> {
    if value::is_null(receiver) || value::is_undefined(receiver) {
        return Err(type_error(
            ctx,
            state,
            "String method called on null or undefined",
        ));
    }
    to_string_coerced(ctx, state, receiver)
}

fn string_normalize(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let text = match require_string(ctx, state, receiver) {
        Ok(text) => text,
        Err(exception) => return exception,
    };
    let form = if args.first().is_none_or(|form| value::is_undefined(*form)) {
        "NFC".to_owned()
    } else {
        match to_string_coerced(ctx, state, args[0]) {
            Ok(form) => form,
            Err(exception) => return exception,
        }
    };
    let form = match form.as_str() {
        "NFC" => NormalizationForm::Nfc,
        "NFD" => NormalizationForm::Nfd,
        "NFKC" => NormalizationForm::Nfkc,
        "NFKD" => NormalizationForm::Nfkd,
        _ => {
            return crate::dispatch::runtime::range_error(
                ctx,
                state,
                "The normalization form should be one of NFC, NFD, NFKC, NFKD",
            );
        }
    };
    intern(ctx, state, normalize(&text, form))
}

fn string_case(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    uppercase: bool,
    locale: Option<&str>,
) -> i64 {
    let text = match require_string(ctx, state, receiver) {
        Ok(text) => text,
        Err(exception) => return exception,
    };
    let mapped = match locale {
        Some(locale) => match locale_case_map(&text, locale, uppercase) {
            Ok(mapped) => mapped,
            Err(error) => return crate::dispatch::runtime::range_error(ctx, state, &error),
        },
        None => case_map(&text, uppercase),
    };
    intern(ctx, state, mapped)
}

fn string_locale_case(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
    uppercase: bool,
) -> i64 {
    let locales = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let requested = match canonicalize_locales(ctx, state, locales) {
        Ok(requested) => requested,
        Err(exception) => return exception,
    };
    let locale = requested
        .first()
        .cloned()
        .unwrap_or_else(wjsm_intl_data::default_locale);
    string_case(ctx, state, receiver, uppercase, Some(&locale))
}

fn locale_compare(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let left = match require_string(ctx, state, receiver) {
        Ok(text) => intern(ctx, state, text),
        Err(exception) => return exception,
    };
    let right = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let locales = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let options = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    collator::compare_with_options(ctx, state, locales, options, left, right)
}

fn number_to_locale(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
    bigint: bool,
) -> i64 {
    let value = if bigint {
        if value::is_bigint(receiver) {
            receiver
        } else if value::is_js_object(receiver)
            && let Some(primitive) = state.boxed_primitives.get(&value::decode_handle(receiver))
            && value::is_bigint(*primitive)
        {
            *primitive
        } else {
            return type_error(
                ctx,
                state,
                "BigInt.prototype.toLocaleString called on incompatible receiver",
            );
        }
    } else if value::is_f64(receiver) {
        receiver
    } else if value::is_js_object(receiver)
        && let Some(primitive) = state.boxed_primitives.get(&value::decode_handle(receiver))
        && value::is_f64(*primitive)
    {
        *primitive
    } else {
        return type_error(
            ctx,
            state,
            "Number.prototype.toLocaleString called on incompatible receiver",
        );
    };
    let locales = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    number_format::format_with_options(ctx, state, locales, options, value)
}

fn array_to_locale(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    if value::is_null(receiver) || value::is_undefined(receiver) {
        return type_error(
            ctx,
            state,
            "Array.prototype.toLocaleString called on null or undefined",
        );
    }
    let locales = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if state
        .typed_arrays
        .contains_key(&value::decode_handle(receiver))
    {
        return typed_array_to_locale(ctx, state, receiver, locales, options);
    }
    let length = if value::is_array(receiver) {
        state
            .gc
            .heap()
            .array_length(value::decode_handle(receiver))
            .unwrap_or(0)
    } else {
        match super::js::get_named(ctx, state, receiver, "length") {
            Ok(length) => match to_number_coerced(ctx, state, length) {
                Ok(length) if length.is_finite() && length > 0.0 => length as u32,
                Ok(_) => 0,
                Err(exception) => return exception,
            },
            Err(exception) => return exception,
        }
    };
    let mut parts = Vec::new();
    for index in 0..length {
        let key = intern(ctx, state, index.to_string());
        if value::is_array(receiver) {
            match state
                .gc
                .heap()
                .get_element(value::decode_handle(receiver), index)
            {
                Ok(Some(element)) if !value::is_array_hole(element as i64) => {
                    match element_locale_string(ctx, state, element as i64, locales, options) {
                        Ok(text) => parts.push(text),
                        Err(exception) => return exception,
                    }
                }
                Ok(_) => parts.push(String::new()),
                Err(_) => return fail_dispatch(ctx),
            }
            continue;
        }
        match has_property(ctx, state, receiver, key) {
            Ok(true) => {}
            Ok(false) => {
                parts.push(String::new());
                continue;
            }
            Err(exception) => return exception,
        }
        let item = match get_property(ctx, state, receiver, key) {
            Ok(item) if !value::is_exception(item) => item,
            Ok(item) => return item,
            Err(()) => return fail_dispatch(ctx),
        };
        match element_locale_string(ctx, state, item, locales, options) {
            Ok(text) => parts.push(text),
            Err(exception) => return exception,
        }
    }
    intern(ctx, state, parts.join(","))
}

fn typed_array_to_locale(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    locales: i64,
    options: i64,
) -> i64 {
    let length = state
        .typed_arrays
        .get(&value::decode_handle(receiver))
        .map(|array| array.length)
        .unwrap_or(0);
    let mut parts = Vec::with_capacity(length);
    for index in 0..length {
        let Some(element) = crate::dispatch::typedarray::get_element_intern(state, receiver, index)
        else {
            parts.push(String::new());
            continue;
        };
        match element_locale_string(ctx, state, element, locales, options) {
            Ok(text) => parts.push(text),
            Err(exception) => return exception,
        }
    }
    intern(ctx, state, parts.join(","))
}

fn element_locale_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    element: i64,
    locales: i64,
    options: i64,
) -> Result<String, i64> {
    if value::is_null(element) || value::is_undefined(element) {
        return Ok(String::new());
    }
    let key = intern(ctx, state, "toLocaleString");
    let method = match get_property(ctx, state, element, key) {
        Ok(method) if !value::is_exception(method) => method,
        Ok(method) => return Err(method),
        Err(()) => return Err(fail_dispatch(ctx)),
    };
    if value::is_callable(method) {
        let result = state
            .invoke_callable(ctx, method, element, &[locales, options])
            .ok_or_else(|| fail_dispatch(ctx))?;
        if value::is_exception(result) {
            return Err(result);
        }
        return to_string_coerced(ctx, state, result);
    }
    to_string_coerced(ctx, state, element)
}

pub(crate) fn date_to_locale(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    milliseconds: f64,
    args: &[i64],
    kind: DateLocaleKind,
) -> i64 {
    if !milliseconds.is_finite() {
        return intern(ctx, state, "Invalid Date");
    }
    let locales = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let options = match date_options(ctx, state, options, kind) {
        Ok(options) => options,
        Err(exception) => return exception,
    };
    let date = match crate::dispatch::date::from_millis(state, milliseconds) {
        Some(date) => date,
        None => return fail_dispatch(ctx),
    };
    datetime_format::format_with_options(
        ctx,
        state,
        locales,
        options,
        date,
        locale_required(kind),
        locale_defaults(kind),
    )
}

fn date_options(
    _ctx: &mut NativeVmContext,
    _state: &mut NativeAgentState,
    options: i64,
    _kind: DateLocaleKind,
) -> Result<i64, i64> {
    Ok(options)
}

fn locale_required(kind: DateLocaleKind) -> datetime_format::DateTimeRequired {
    match kind {
        DateLocaleKind::Date => datetime_format::DateTimeRequired::Date,
        DateLocaleKind::Time => datetime_format::DateTimeRequired::Time,
        DateLocaleKind::DateTime => datetime_format::DateTimeRequired::Any,
    }
}

fn locale_defaults(kind: DateLocaleKind) -> datetime_format::DateTimeDefaults {
    match kind {
        DateLocaleKind::Date => datetime_format::DateTimeDefaults::Date,
        DateLocaleKind::Time => datetime_format::DateTimeDefaults::Time,
        DateLocaleKind::DateTime => datetime_format::DateTimeDefaults::All,
    }
}

#[derive(Clone, Copy)]
pub(crate) enum DateLocaleKind {
    DateTime,
    Date,
    Time,
}

pub(crate) fn primitive_locale_property(
    state: &mut NativeAgentState,
    receiver: i64,
    key: &str,
) -> Option<i64> {
    let prototype = if value::is_string(receiver)
        && matches!(
            key,
            "normalize"
                | "toLowerCase"
                | "toUpperCase"
                | "toLocaleLowerCase"
                | "toLocaleUpperCase"
                | "localeCompare"
        ) {
        ensure_string_prototype(state)?
    } else if value::is_f64(receiver) && key == "toLocaleString" {
        ensure_number_prototype(state)?
    } else if value::is_bigint(receiver) && key == "toLocaleString" {
        ensure_bigint_prototype(state)?
    } else if key == "toLocaleString"
        && value::is_js_object(receiver)
        && state
            .typed_arrays
            .contains_key(&value::decode_handle(receiver))
    {
        return state.native_callable(NativeCallableKind::Intl(IntlCallable::ArrayToLocaleString));
    } else if value::is_array(receiver) && key == "toLocaleString" {
        let prototype = state.array_prototype?;
        install_array_to_locale_string(state, prototype).ok()?;
        let key = state.intern_property_string("toLocaleString".into())?;
        return state
            .array_properties
            .get(&(value::decode_handle(prototype), key))
            .copied();
    } else {
        return None;
    };
    let key = state.intern_property_string(key.into())?;
    state
        .gc
        .heap()
        .get_property(value::decode_handle(prototype), key)
        .ok()
        .flatten()
        .map(|stored| stored as i64)
}

pub(crate) fn ensure_string_prototype(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(prototype) = state.intl.string_prototype {
        return Some(prototype);
    }
    let constructor = state.native_callable(crate::NativeCallableKind::StringConstructor)?;
    let prototype = allocate_proto(state, constructor)?;
    for (name, kind) in [
        ("normalize", IntlCallable::StringNormalize),
        ("toLowerCase", IntlCallable::StringToLowerCase),
        ("toUpperCase", IntlCallable::StringToUpperCase),
        ("toLocaleLowerCase", IntlCallable::StringToLocaleLowerCase),
        ("toLocaleUpperCase", IntlCallable::StringToLocaleUpperCase),
        ("localeCompare", IntlCallable::StringLocaleCompare),
    ] {
        super::install::install_method(state, prototype, name, kind).ok()?;
    }
    for (name, builtin) in [
        ("toString", Builtin::StringToString),
        ("valueOf", Builtin::StringValueOf),
    ] {
        let callable = state.native_callable(crate::NativeCallableKind::Builtin(builtin, true))?;
        super::install::install_data_property(
            state,
            prototype,
            name,
            callable,
            crate::BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
        )
        .ok()?;
    }
    state.intl.string_prototype = Some(prototype);
    Some(prototype)
}

pub(crate) fn ensure_number_prototype(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(prototype) = state.intl.number_prototype {
        return Some(prototype);
    }
    let constructor = state.native_callable(crate::NativeCallableKind::Builtin(
        wjsm_ir::Builtin::NumberConstructor,
        false,
    ))?;
    let prototype = allocate_proto(state, constructor)?;
    super::install::install_method(
        state,
        prototype,
        "toLocaleString",
        IntlCallable::NumberToLocaleString,
    )
    .ok()?;
    state.intl.number_prototype = Some(prototype);
    Some(prototype)
}

pub(crate) fn ensure_bigint_prototype(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(prototype) = state.intl.bigint_prototype {
        return Some(prototype);
    }
    let constructor = state.native_callable(crate::NativeCallableKind::Builtin(
        wjsm_ir::Builtin::BigIntFromLiteral,
        false,
    ))?;
    let prototype = allocate_proto(state, constructor)?;
    super::install::install_method(
        state,
        prototype,
        "toLocaleString",
        IntlCallable::BigIntToLocaleString,
    )
    .ok()?;
    state.intl.bigint_prototype = Some(prototype);
    Some(prototype)
}

pub(crate) fn install_array_to_locale_string(
    state: &mut NativeAgentState,
    prototype: i64,
) -> Result<(), ()> {
    let callable = state
        .native_callable(crate::NativeCallableKind::Intl(
            IntlCallable::ArrayToLocaleString,
        ))
        .ok_or(())?;
    let key = state
        .intern_property_string("toLocaleString".into())
        .ok_or(())?;
    let handle = value::decode_handle(prototype);
    if state.array_properties.contains_key(&(handle, key)) {
        return Ok(());
    }
    state.note_array_property(handle, key);
    state.array_properties.insert((handle, key), callable);
    state
        .array_property_flags
        .insert((handle, key), crate::BUILTIN_PROTOTYPE_PROPERTY_FLAGS);
    Ok(())
}

fn allocate_proto(state: &mut NativeAgentState, constructor: i64) -> Option<i64> {
    let prototype = state.allocate_object(4, false).ok()?;
    let constructor_key = state.intern_property_string("constructor".into())?;
    state
        .gc
        .heap()
        .define_data_property(
            value::decode_handle(prototype),
            constructor_key,
            constructor as u64,
            crate::BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
        )
        .ok()?;
    let prototype_key = state.intern_property_string("prototype".into())?;
    state
        .callable_properties
        .insert((constructor, prototype_key), prototype);
    state.callable_property_flags.insert(
        (constructor, prototype_key),
        crate::FUNCTION_PROTOTYPE_FLAGS,
    );
    Some(prototype)
}
