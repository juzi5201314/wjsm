//! `Intl.DisplayNames`。

use wjsm_builtins::intl::resolve_locale;
use wjsm_intl_data::{DisplayNameType, OwnedDisplayNames};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::common::{create_instance, intern, slot_handle, throw_intl};
use super::js::{
    canonicalize_locales, get_option_string, get_options_object, supported_locales_of,
};
use super::slots::{DisplayNamesSlot, IntlSlot};
use super::{IntlCallable, incompatible};
use crate::NativeAgentState;
use crate::dispatch::runtime::{fail_dispatch, range_error, to_string_coerced, type_error};

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: IntlCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    match callable {
        IntlCallable::DisplayNamesConstructor => construct(ctx, state, receiver, args),
        IntlCallable::DisplayNamesSupportedLocalesOf => supported_locales_of(ctx, state, args),
        IntlCallable::DisplayNamesResolvedOptions => resolved(ctx, state, receiver),
        IntlCallable::DisplayNamesOf => of(ctx, state, receiver, args),
        _ => fail_dispatch(ctx),
    }
}

fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    if let Err(exception) = super::common::require_new(ctx, state) {
        return exception;
    }
    let requested = match canonicalize_locales(
        ctx,
        state,
        args.first()
            .copied()
            .unwrap_or_else(value::encode_undefined),
    ) {
        Ok(requested) => requested,
        Err(exception) => return exception,
    };
    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if value::is_undefined(options) {
        return type_error(ctx, state, "Intl.DisplayNames requires options.type");
    }
    let options = match get_options_object(ctx, state, options) {
        Ok(options) => options,
        Err(exception) => return exception,
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
    let style = match get_option_string(
        ctx,
        state,
        options,
        "style",
        &["narrow", "short", "long"],
        Some("long"),
    ) {
        Ok(value) => value.unwrap_or_else(|| "long".into()),
        Err(exception) => return exception,
    };
    let type_name = match get_option_string(
        ctx,
        state,
        options,
        "type",
        &[
            "language",
            "region",
            "script",
            "currency",
            "calendar",
            "dateTimeField",
        ],
        None,
    ) {
        Ok(Some(value)) => value,
        Ok(None) => return type_error(ctx, state, "options.type is required"),
        Err(exception) => return exception,
    };
    let fallback = match get_option_string(
        ctx,
        state,
        options,
        "fallback",
        &["code", "none"],
        Some("code"),
    ) {
        Ok(value) => value.unwrap_or_else(|| "code".into()),
        Err(exception) => return exception,
    };
    let language_display = match get_option_string(
        ctx,
        state,
        options,
        "languageDisplay",
        &["dialect", "standard"],
        Some("dialect"),
    ) {
        Ok(value) => value.unwrap_or_else(|| "dialect".into()),
        Err(exception) => return exception,
    };
    let resolved = match resolve_locale(&requested, &[], &Default::default()) {
        Ok(resolved) => resolved,
        Err(error) => return throw_intl(ctx, state, error),
    };
    create_instance(
        ctx,
        state,
        IntlCallable::DisplayNamesConstructor,
        IntlSlot::DisplayNames(DisplayNamesSlot {
            locale: resolved.locale,
            style,
            type_name,
            fallback,
            language_display,
            formatter: None,
        }),
        this_value,
    )
}

fn resolved(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::DisplayNames(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let pairs = [
        ("locale", slot.locale.clone()),
        ("style", slot.style.clone()),
        ("type", slot.type_name.clone()),
        ("fallback", slot.fallback.clone()),
        ("languageDisplay", slot.language_display.clone()),
    ];
    super::common::resolved_strings(ctx, state, &pairs)
}

fn of(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64, args: &[i64]) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let code = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let code = match to_string_coerced(ctx, state, code) {
        Ok(code) => code,
        Err(exception) => return exception,
    };
    let Some(IntlSlot::DisplayNames(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    if let Err(error) = canonical_code_for_display_names(&slot.type_name, &code) {
        return range_error(ctx, state, &error);
    }
    if let Err(exception) = ensure(ctx, state, handle) {
        return exception;
    }
    let Some(IntlSlot::DisplayNames(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    match slot
        .formatter
        .as_ref()
        .and_then(|formatter| formatter.of(&code).ok())
        .flatten()
    {
        Some(name) => intern(ctx, state, name),
        None if slot.fallback == "none" => value::encode_undefined(),
        None => intern(ctx, state, code),
    }
}

fn ensure(ctx: &mut NativeVmContext, state: &mut NativeAgentState, handle: u32) -> Result<(), i64> {
    let Some(IntlSlot::DisplayNames(slot)) = state.intl.slots.get(&handle) else {
        return Err(incompatible(ctx, state));
    };
    if slot.formatter.is_some() {
        return Ok(());
    }
    let kind = match slot.type_name.as_str() {
        "language" => DisplayNameType::Language,
        "region" => DisplayNameType::Region,
        "script" => DisplayNameType::Script,
        "currency" => DisplayNameType::Currency,
        "calendar" => DisplayNameType::Calendar,
        _ => DisplayNameType::DateTimeField,
    };
    let locale = slot.locale.clone();
    let formatter = OwnedDisplayNames::try_new(&locale, kind)
        .map_err(|error| range_error(ctx, state, &error))?;
    if let Some(IntlSlot::DisplayNames(slot)) = state.intl.slots.get_mut(&handle) {
        slot.formatter = Some(formatter);
    }
    Ok(())
}

fn canonical_code_for_display_names(type_name: &str, code: &str) -> Result<String, String> {
    match type_name {
        "language" => {
            if !wjsm_intl_data::is_unicode_language_id(code)
                || !wjsm_intl_data::is_structurally_valid_language_tag(code)
            {
                return Err("invalid language code".into());
            }
            wjsm_intl_data::canonicalize_unicode_locale_id(code)
        }
        "region" => {
            let valid = (code.len() == 2 && code.bytes().all(|byte| byte.is_ascii_alphabetic()))
                || (code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_digit()));
            if valid {
                Ok(code.to_ascii_uppercase())
            } else {
                Err("invalid region code".into())
            }
        }
        "script" => {
            if code.len() == 4 && code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                Ok(code.to_owned())
            } else {
                Err("invalid script code".into())
            }
        }
        "currency" => {
            if code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                Ok(code.to_ascii_uppercase())
            } else {
                Err("invalid currency code".into())
            }
        }
        "calendar" => {
            if super::js::is_unicode_type(code) {
                Ok(code.to_ascii_lowercase())
            } else {
                Err("invalid calendar code".into())
            }
        }
        _ => {
            const FIELDS: &[&str] = &[
                "era",
                "year",
                "quarter",
                "month",
                "weekOfYear",
                "weekday",
                "day",
                "dayPeriod",
                "hour",
                "minute",
                "second",
                "timeZoneName",
            ];
            if FIELDS.contains(&code) {
                Ok(code.to_owned())
            } else {
                Err("invalid dateTimeField code".into())
            }
        }
    }
}
