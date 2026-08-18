//! `Intl.NumberFormat`。

use wjsm_intl_data::{FormatPart, NumberFormatSpec, OwnedNumberFormatter};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::common::{create_instance, intern, resolved_object, slot_handle};
use super::js::{canonicalize_locales, get_options_object, supported_locales_of, to_object};
use super::number_format_options::read_options;
use super::slots::IntlSlot;
use super::{IntlCallable, incompatible};
use crate::NativeAgentState;
use crate::NativeCallableKind;
use crate::dispatch::runtime::{
    fail_dispatch, range_error, to_number_coerced, to_string_coerced, type_error,
};

pub(super) fn format_with_options(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    locales: i64,
    options: i64,
    value: i64,
) -> i64 {
    let instance = construct(ctx, state, value::encode_undefined(), &[locales, options]);
    if value::is_exception(instance) {
        return instance;
    }
    format_value(ctx, state, value::decode_handle(instance), &[value], false)
}

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: IntlCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    match callable {
        IntlCallable::NumberFormatConstructor => construct(ctx, state, receiver, args),
        IntlCallable::NumberFormatSupportedLocalesOf => supported_locales_of(ctx, state, args),
        IntlCallable::NumberFormatResolvedOptions => resolved(ctx, state, receiver),
        IntlCallable::NumberFormatFormatGet => format_get(ctx, state, receiver),
        IntlCallable::NumberFormatFormat(handle) => format_value(ctx, state, handle, args, false),
        IntlCallable::NumberFormatFormatToParts => {
            let handle = match slot_handle(receiver) {
                Some(handle) => handle,
                None => return incompatible(ctx, state),
            };
            format_value(ctx, state, handle, args, true)
        }
        IntlCallable::NumberFormatFormatRange => format_range(ctx, state, receiver, args, false),
        IntlCallable::NumberFormatFormatRangeToParts => {
            format_range(ctx, state, receiver, args, true)
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
    let locales = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let requested = match canonicalize_locales(ctx, state, locales) {
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
    match read_options(ctx, state, &requested, options) {
        Ok(slot) => create_instance(
            ctx,
            state,
            IntlCallable::NumberFormatConstructor,
            IntlSlot::NumberFormat(slot),
            this_value,
        ),
        Err(exception) => exception,
    }
}

fn resolved(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::NumberFormat(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let locale = slot.locale.clone();
    let numbering_system = slot.numbering_system.clone();
    let style = slot.style.clone();
    let min_int = slot.minimum_integer_digits;
    let min_frac = slot.minimum_fraction_digits;
    let max_frac = slot.maximum_fraction_digits;
    let use_grouping = slot.use_grouping.clone();
    let notation = slot.notation.clone();
    let sign_display = slot.sign_display.clone();
    let rounding_mode = slot.rounding_mode.clone();
    let rounding_increment = slot.rounding_increment;
    let rounding_priority = slot.rounding_priority.clone();
    let trailing_zero_display = slot.trailing_zero_display.clone();
    let currency = slot.currency.clone();
    let currency_display = slot.currency_display.clone();
    let currency_sign = slot.currency_sign.clone();
    let unit = slot.unit.clone();
    let unit_display = slot.unit_display.clone();
    let compact_display = slot.compact_display.clone();
    let min_sig = slot.minimum_significant_digits;
    let max_sig = slot.maximum_significant_digits;
    let locale = intern(ctx, state, locale);
    let numbering_system = intern(ctx, state, numbering_system);
    let style_v = intern(ctx, state, style.clone());
    let use_grouping = if use_grouping == "false" {
        value::encode_bool(false)
    } else {
        intern(ctx, state, use_grouping)
    };
    let notation_v = intern(ctx, state, notation.clone());
    let sign_display = intern(ctx, state, sign_display);
    let rounding_mode = intern(ctx, state, rounding_mode);
    let trailing_zero_display = intern(ctx, state, trailing_zero_display);
    let show_fraction = rounding_priority != "auto" || min_sig.is_none();
    let mut fields = vec![
        ("locale", locale),
        ("numberingSystem", numbering_system),
        ("style", style_v),
    ];
    if let Some(currency) = currency {
        fields.push(("currency", intern(ctx, state, currency)));
        fields.push(("currencyDisplay", intern(ctx, state, currency_display)));
        fields.push(("currencySign", intern(ctx, state, currency_sign)));
    }
    if let Some(unit) = unit {
        fields.push(("unit", intern(ctx, state, unit)));
        fields.push(("unitDisplay", intern(ctx, state, unit_display)));
    }
    fields.push(("minimumIntegerDigits", value::encode_f64(min_int as f64)));
    if show_fraction {
        fields.push(("minimumFractionDigits", value::encode_f64(min_frac as f64)));
        fields.push(("maximumFractionDigits", value::encode_f64(max_frac as f64)));
    }
    if let (Some(min_sig), Some(max_sig)) = (min_sig, max_sig) {
        fields.push((
            "minimumSignificantDigits",
            value::encode_f64(min_sig as f64),
        ));
        fields.push((
            "maximumSignificantDigits",
            value::encode_f64(max_sig as f64),
        ));
    }
    fields.push(("useGrouping", use_grouping));
    fields.push(("notation", notation_v));
    if notation == "compact" {
        fields.push(("compactDisplay", intern(ctx, state, compact_display)));
    }
    fields.push(("signDisplay", sign_display));
    fields.push((
        "roundingIncrement",
        value::encode_f64(rounding_increment as f64),
    ));
    fields.push(("roundingMode", rounding_mode));
    fields.push(("roundingPriority", intern(ctx, state, rounding_priority)));
    fields.push(("trailingZeroDisplay", trailing_zero_display));
    resolved_object(ctx, state, &fields)
}

fn format_get(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::NumberFormat(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    if let Some(bound) = slot.bound_format {
        return bound;
    }
    let Some(bound) = state.native_callable(NativeCallableKind::Intl(
        IntlCallable::NumberFormatFormat(handle),
    )) else {
        return fail_dispatch(ctx);
    };
    if let Some(IntlSlot::NumberFormat(slot)) = state.intl.slots.get_mut(&handle) {
        slot.bound_format = Some(bound);
    }
    bound
}

fn format_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    args: &[i64],
    parts: bool,
) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let text = match numeric_text(ctx, state, input) {
        Ok(text) => text,
        Err(exception) => return exception,
    };
    if let Err(exception) = ensure_formatter(ctx, state, handle) {
        return exception;
    }
    let Some(IntlSlot::NumberFormat(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let Some(formatter) = slot.formatter.as_ref() else {
        return fail_dispatch(ctx);
    };
    let rendered = match formatter.format_str(&text) {
        Ok(rendered) => rendered,
        Err(_) => text.clone(),
    };
    if parts {
        return super::common::parts_array(
            ctx,
            state,
            formatter.format_parts_str(&text).unwrap_or_else(|_| {
                vec![FormatPart {
                    type_name: "integer".into(),
                    value: rendered.clone(),
                    source: None,
                    unit: None,
                }]
            }),
        );
    }
    intern(ctx, state, rendered)
}

fn format_range(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
    parts: bool,
) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    if args.len() < 2 || value::is_undefined(args[0]) || value::is_undefined(args[1]) {
        return type_error(ctx, state, "formatRange requires two arguments");
    }
    if !value::is_bigint(args[0]) && !value::is_bigint(args[1]) {
        let start_number = match to_number_coerced(ctx, state, args[0]) {
            Ok(number) => number,
            Err(exception) => return exception,
        };
        let end_number = match to_number_coerced(ctx, state, args[1]) {
            Ok(number) => number,
            Err(exception) => return exception,
        };
        if start_number.is_nan() || end_number.is_nan() {
            return range_error(ctx, state, "formatRange start is after end");
        }
    }
    let start = match numeric_text(ctx, state, args[0]) {
        Ok(text) => text,
        Err(exception) => return exception,
    };
    let end = match numeric_text(ctx, state, args[1]) {
        Ok(text) => text,
        Err(exception) => return exception,
    };
    if let Err(exception) = ensure_formatter(ctx, state, handle) {
        return exception;
    }
    let Some(IntlSlot::NumberFormat(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let Some(formatter) = slot.formatter.as_ref() else {
        return fail_dispatch(ctx);
    };
    if parts {
        return super::common::parts_array(
            ctx,
            state,
            formatter
                .format_range_parts_str(&start, &end)
                .unwrap_or_default(),
        );
    }
    match formatter.format_range_str(&start, &end) {
        Ok(rendered) => intern(ctx, state, rendered),
        Err(_) => fail_dispatch(ctx),
    }
}

fn numeric_text(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    input: i64,
) -> Result<String, i64> {
    if value::is_bigint(input) {
        return to_string_coerced(ctx, state, input);
    }
    if value::is_string(input) {
        let text = to_string_coerced(ctx, state, input)?;
        return Ok(intl_math_string(&text));
    }
    let number = to_number_coerced(ctx, state, input)?;
    if number.is_nan() {
        return Ok("NaN".into());
    }
    if number.is_infinite() {
        return Ok(if number.is_sign_negative() {
            "-Infinity".into()
        } else {
            "Infinity".into()
        });
    }
    Ok(format!("{number}"))
}

fn intl_math_string(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("nan") {
        return "NaN".into();
    }
    if trimmed.eq_ignore_ascii_case("infinity") {
        return "Infinity".into();
    }
    if trimmed.eq_ignore_ascii_case("+infinity") {
        return "Infinity".into();
    }
    if trimmed.eq_ignore_ascii_case("-infinity") {
        return "-Infinity".into();
    }
    if is_decimal_string(trimmed) {
        return trimmed.to_owned();
    }
    "NaN".into()
}

fn is_decimal_string(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut index = 0;
    if matches!(bytes[0], b'+' | b'-') {
        index = 1;
    }
    if index >= bytes.len() {
        return false;
    }
    let mut saw_digit = false;
    let mut saw_dot = false;
    while index < bytes.len() {
        match bytes[index] {
            b'0'..=b'9' => saw_digit = true,
            b'.' if !saw_dot => saw_dot = true,
            _ => return false,
        }
        index += 1;
    }
    saw_digit
}

fn ensure_formatter(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
) -> Result<(), i64> {
    let Some(IntlSlot::NumberFormat(slot)) = state.intl.slots.get(&handle) else {
        return Err(incompatible(ctx, state));
    };
    if slot.formatter.is_some() {
        return Ok(());
    }
    let spec = NumberFormatSpec {
        locale: slot.locale.clone(),
        numbering_system: slot.numbering_system.clone(),
        style: slot.style.clone(),
        currency: slot.currency.clone(),
        currency_display: slot.currency_display.clone(),
        currency_sign: slot.currency_sign.clone(),
        unit: slot.unit.clone(),
        unit_display: slot.unit_display.clone(),
        notation: slot.notation.clone(),
        compact_display: slot.compact_display.clone(),
        sign_display: slot.sign_display.clone(),
        use_grouping: if slot.notation == "compact" && slot.use_grouping == "auto" {
            "min2".into()
        } else {
            slot.use_grouping.clone()
        },
        minimum_integer_digits: slot.minimum_integer_digits,
        minimum_fraction_digits: slot.minimum_fraction_digits,
        maximum_fraction_digits: slot.maximum_fraction_digits,
        minimum_significant_digits: slot.minimum_significant_digits,
        maximum_significant_digits: slot.maximum_significant_digits,
        rounding_mode: slot.rounding_mode.clone(),
        rounding_increment: slot.rounding_increment,
        rounding_priority: slot.rounding_priority.clone(),
        trailing_zero_display: slot.trailing_zero_display.clone(),
    };
    let formatter =
        OwnedNumberFormatter::try_new(spec).map_err(|error| range_error(ctx, state, &error))?;
    if let Some(IntlSlot::NumberFormat(slot)) = state.intl.slots.get_mut(&handle) {
        slot.formatter = Some(formatter);
    }
    Ok(())
}
