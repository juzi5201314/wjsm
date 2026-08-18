//! `Intl.RelativeTimeFormat`。

use std::collections::BTreeMap;

use wjsm_builtins::intl::resolve_locale_filtered;
use wjsm_intl_data::{OwnedRelativeTimeFormatter, available_numbering_systems};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::common::{create_instance, intern, parts_array, slot_handle, throw_intl};
use super::js::{
    canonicalize_locales, get_option_string, get_options_object, supported_locales_of, to_object,
};
use super::slots::{IntlSlot, RelativeTimeSlot};
use super::{IntlCallable, incompatible};
use crate::NativeAgentState;
use crate::dispatch::runtime::{fail_dispatch, range_error, to_number_coerced, to_string_coerced};

const UNITS: &[&str] = &[
    "year", "years", "quarter", "quarters", "month", "months", "week", "weeks", "day", "days",
    "hour", "hours", "minute", "minutes", "second", "seconds",
];

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: IntlCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    match callable {
        IntlCallable::RelativeTimeFormatConstructor => construct(ctx, state, receiver, args),
        IntlCallable::RelativeTimeFormatSupportedLocalesOf => {
            supported_locales_of(ctx, state, args)
        }
        IntlCallable::RelativeTimeFormatResolvedOptions => resolved(ctx, state, receiver),
        IntlCallable::RelativeTimeFormatFormat => format_rel(ctx, state, receiver, args, false),
        IntlCallable::RelativeTimeFormatFormatToParts => {
            format_rel(ctx, state, receiver, args, true)
        }
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
    let raw_options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let options = if value::is_undefined(raw_options) {
        match get_options_object(ctx, state, raw_options) {
            Ok(options) => options,
            Err(exception) => return exception,
        }
    } else {
        match to_object(ctx, state, raw_options) {
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
    let numbering_system =
        match super::js::require_unicode_type(ctx, state, options, "numberingSystem") {
            Ok(value) => value,
            Err(exception) => return exception,
        };
    let style = match get_option_string(
        ctx,
        state,
        options,
        "style",
        &["long", "short", "narrow"],
        Some("long"),
    ) {
        Ok(value) => value.unwrap_or_else(|| "long".into()),
        Err(exception) => return exception,
    };
    let numeric = match get_option_string(
        ctx,
        state,
        options,
        "numeric",
        &["always", "auto"],
        Some("always"),
    ) {
        Ok(value) => value.unwrap_or_else(|| "always".into()),
        Err(exception) => return exception,
    };
    let mut ext = BTreeMap::new();
    if let Some(system) = numbering_system
        .as_deref()
        .filter(|system| available_numbering_systems().contains(system))
    {
        ext.insert("nu".into(), system.to_owned());
    }
    let resolved = match resolve_locale_filtered(&requested, &["nu"], &ext, |key, value, _| {
        key == "nu" && available_numbering_systems().contains(&value)
    }) {
        Ok(resolved) => resolved,
        Err(error) => return throw_intl(ctx, state, error),
    };
    create_instance(
        ctx,
        state,
        IntlCallable::RelativeTimeFormatConstructor,
        IntlSlot::RelativeTime(RelativeTimeSlot {
            locale: resolved.locale,
            numbering_system: resolved
                .extensions
                .get("nu")
                .cloned()
                .unwrap_or_else(|| "latn".into()),
            numeric,
            style,
            formatter: None,
        }),
        this_value,
    )
}

fn resolved(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::RelativeTime(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let pairs = [
        ("locale", slot.locale.clone()),
        ("style", slot.style.clone()),
        ("numeric", slot.numeric.clone()),
        ("numberingSystem", slot.numbering_system.clone()),
    ];
    super::common::resolved_strings(ctx, state, &pairs)
}

fn format_rel(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
    parts: bool,
) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    if !matches!(
        state.intl.slots.get(&handle),
        Some(IntlSlot::RelativeTime(_))
    ) {
        return incompatible(ctx, state);
    }
    let value = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let unit = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let number = match to_number_coerced(ctx, state, value) {
        Ok(number) => number,
        Err(exception) => return exception,
    };
    if !number.is_finite() {
        return range_error(ctx, state, "value must be finite");
    }
    let unit = match to_string_coerced(ctx, state, unit) {
        Ok(unit) => unit,
        Err(exception) => return exception,
    };
    if !UNITS.contains(&unit.as_str()) {
        return range_error(ctx, state, "invalid relative time unit");
    }
    let unit = unit.trim_end_matches('s');
    if let Err(exception) = ensure(ctx, state, handle) {
        return exception;
    }
    let Some(IntlSlot::RelativeTime(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let Some(formatter) = slot.formatter.as_ref() else {
        return fail_dispatch(ctx);
    };
    if parts {
        return match formatter.format_parts(number, unit) {
            Ok(parts) => parts_array(ctx, state, parts),
            Err(error) => range_error(ctx, state, &error),
        };
    }
    match formatter.format(number, unit) {
        Ok(text) => intern(ctx, state, text),
        Err(error) => range_error(ctx, state, &error),
    }
}

fn ensure(ctx: &mut NativeVmContext, state: &mut NativeAgentState, handle: u32) -> Result<(), i64> {
    let Some(IntlSlot::RelativeTime(slot)) = state.intl.slots.get(&handle) else {
        return Err(incompatible(ctx, state));
    };
    if slot.formatter.is_some() {
        return Ok(());
    }
    let locale = slot.locale.clone();
    let numeric = slot.numeric.clone();
    let style = slot.style.clone();
    let formatter = OwnedRelativeTimeFormatter::try_new(&locale, &numeric, &style)
        .map_err(|error| range_error(ctx, state, &error))?;
    if let Some(IntlSlot::RelativeTime(slot)) = state.intl.slots.get_mut(&handle) {
        slot.formatter = Some(formatter);
    }
    Ok(())
}
