//! `Intl.DurationFormat`。

use wjsm_intl_data::{DurationFormatSpec, DurationUnitSpec, parse_iso_duration};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::common::{create_instance, intern, parts_array, slot_handle, throw_intl};
use super::js::{
    canonicalize_locales, get_named, get_number_option, get_option_string, get_options_object,
    require_unicode_type, supported_locales_of,
};
use super::slots::{DurationFormatSlot, DurationUnitSlot, IntlSlot};
use super::{IntlCallable, incompatible};
use crate::NativeAgentState;
use crate::dispatch::runtime::{
    fail_dispatch, range_error, to_number_coerced, to_string_coerced, type_error,
};

const FIELDS: &[&str] = &[
    "years",
    "months",
    "weeks",
    "days",
    "hours",
    "minutes",
    "seconds",
    "milliseconds",
    "microseconds",
    "nanoseconds",
];

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: IntlCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    match callable {
        IntlCallable::DurationFormatConstructor => construct(ctx, state, receiver, args),
        IntlCallable::DurationFormatSupportedLocalesOf => supported_locales_of(ctx, state, args),
        IntlCallable::DurationFormatResolvedOptions => resolved(ctx, state, receiver),
        IntlCallable::DurationFormatFormat => format_duration(ctx, state, receiver, args, false),
        IntlCallable::DurationFormatFormatToParts => {
            format_duration(ctx, state, receiver, args, true)
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
    let options = match get_options_object(
        ctx,
        state,
        args.get(1).copied().unwrap_or_else(value::encode_undefined),
    ) {
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
    let numbering_system = match require_unicode_type(ctx, state, options, "numberingSystem") {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let style = match get_option_string(
        ctx,
        state,
        options,
        "style",
        &["long", "short", "narrow", "digital"],
        Some("short"),
    ) {
        Ok(value) => value.unwrap_or_else(|| "short".into()),
        Err(exception) => return exception,
    };
    let units = match read_duration_units(ctx, state, options, &style) {
        Ok(units) => units,
        Err(exception) => return exception,
    };
    let fractional_digits =
        match get_number_option(ctx, state, options, "fractionalDigits", 0.0, 9.0, None) {
            Ok(value) => value.map(|value| value as u32),
            Err(exception) => return exception,
        };
    let mut ext = std::collections::BTreeMap::new();
    if let Some(system) = numbering_system
        .as_deref()
        .filter(|system| wjsm_intl_data::available_numbering_systems().contains(system))
    {
        ext.insert("nu".into(), system.to_owned());
    }
    let resolved = match wjsm_builtins::intl::resolve_locale_filtered(
        &requested,
        &["nu"],
        &ext,
        |key, value, _| {
            key == "nu" && wjsm_intl_data::available_numbering_systems().contains(&value)
        },
    ) {
        Ok(resolved) => resolved,
        Err(error) => return throw_intl(ctx, state, error),
    };
    create_instance(
        ctx,
        state,
        IntlCallable::DurationFormatConstructor,
        IntlSlot::DurationFormat(DurationFormatSlot {
            locale: resolved.locale,
            numbering_system: resolved
                .extensions
                .get("nu")
                .cloned()
                .unwrap_or_else(|| "latn".into()),
            style,
            units,
            fractional_digits,
        }),
        this_value,
    )
}

fn resolved(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::DurationFormat(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let locale = slot.locale.clone();
    let numbering_system = slot.numbering_system.clone();
    let style = slot.style.clone();
    let units = slot.units.clone();
    let fractional_digits = slot.fractional_digits;
    let mut fields = vec![
        ("locale", intern(ctx, state, locale)),
        ("numberingSystem", intern(ctx, state, numbering_system)),
        ("style", intern(ctx, state, style)),
    ];
    for (name, unit) in units {
        let display_key = match name.as_str() {
            "years" => "yearsDisplay",
            "months" => "monthsDisplay",
            "weeks" => "weeksDisplay",
            "days" => "daysDisplay",
            "hours" => "hoursDisplay",
            "minutes" => "minutesDisplay",
            "seconds" => "secondsDisplay",
            "milliseconds" => "millisecondsDisplay",
            "microseconds" => "microsecondsDisplay",
            "nanoseconds" => "nanosecondsDisplay",
            _ => continue,
        };
        // resolvedOptions 把内部 "fractional" 暴露成 "numeric"。
        let style = if unit.style == "fractional" {
            "numeric"
        } else {
            unit.style.as_str()
        };
        fields.push((name_key(&name), intern(ctx, state, style)));
        fields.push((display_key, intern(ctx, state, unit.display)));
    }
    if let Some(digits) = fractional_digits {
        fields.push(("fractionalDigits", value::encode_f64(digits as f64)));
    }
    super::common::resolved_object(ctx, state, &fields)
}

fn name_key(name: &str) -> &'static str {
    match name {
        "years" => "years",
        "months" => "months",
        "weeks" => "weeks",
        "days" => "days",
        "hours" => "hours",
        "minutes" => "minutes",
        "seconds" => "seconds",
        "milliseconds" => "milliseconds",
        "microseconds" => "microseconds",
        "nanoseconds" => "nanoseconds",
        _ => "years",
    }
}

fn format_duration(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
    parts: bool,
) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let duration = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let fields = match duration_record(ctx, state, duration) {
        Ok(fields) => fields,
        Err(exception) => return exception,
    };
    let Some(IntlSlot::DurationFormat(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let spec = DurationFormatSpec {
        locale: slot.locale.clone(),
        numbering_system: slot.numbering_system.clone(),
        style: slot.style.clone(),
        units: slot
            .units
            .iter()
            .map(|(_, unit)| DurationUnitSpec {
                style: unit.style.clone(),
                display: unit.display.clone(),
            })
            .collect(),
        fractional_digits: slot.fractional_digits,
    };
    if parts {
        return match spec.format_parts(&fields) {
            Ok(parts) => parts_array(ctx, state, parts),
            Err(error) => range_error(ctx, state, &error),
        };
    }
    match spec.format(&fields) {
        Ok(text) => intern(ctx, state, text),
        Err(error) => range_error(ctx, state, &error),
    }
}

fn duration_record(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    duration: i64,
) -> Result<[f64; 10], i64> {
    if value::is_string(duration) {
        let text = to_string_coerced(ctx, state, duration)?;
        return parse_iso_duration(&text).map_err(|error| range_error(ctx, state, &error));
    }
    if !super::common::is_type_object(duration) {
        return Err(type_error(ctx, state, "duration must be an object"));
    }
    let mut fields = [0f64; 10];
    let mut seen = false;
    for (index, field) in FIELDS.iter().enumerate() {
        let stored = get_named(ctx, state, duration, field)?;
        if value::is_undefined(stored) {
            continue;
        }
        seen = true;
        let number = to_number_coerced(ctx, state, stored)?;
        if !number.is_finite() || number.fract() != 0.0 {
            return Err(range_error(ctx, state, "invalid duration field"));
        }
        fields[index] = number;
    }
    if !seen {
        return Err(type_error(ctx, state, "duration is missing fields"));
    }
    let mut sign = 0i8;
    for value in fields {
        if value == 0.0 {
            continue;
        }
        let next = if value < 0.0 { -1 } else { 1 };
        if sign == 0 {
            sign = next;
        } else if sign != next {
            return Err(range_error(ctx, state, "invalid duration field"));
        }
    }
    if !valid_duration(&fields) {
        return Err(range_error(ctx, state, "duration is out of range"));
    }
    Ok(fields)
}

fn valid_duration(fields: &[f64; 10]) -> bool {
    if fields[0].abs() >= 4_294_967_296.0
        || fields[1].abs() >= 4_294_967_296.0
        || fields[2].abs() >= 4_294_967_296.0
    {
        return false;
    }
    // 用整数纳秒比较，避免 f64 把合法的 2^53 边界加成分数后误判越界。
    let factors = [
        86_400i128 * 1_000_000_000,
        3_600i128 * 1_000_000_000,
        60i128 * 1_000_000_000,
        1_000_000_000,
        1_000_000,
        1_000,
        1,
    ];
    let mut total_ns = 0i128;
    for (value, factor) in fields[3..].iter().zip(factors) {
        let Some(unit) = f64_to_i128(*value) else {
            return false;
        };
        total_ns = match total_ns.checked_add(unit.checked_mul(factor).unwrap_or(i128::MAX)) {
            Some(total) => total,
            None => return false,
        };
    }
    total_ns.unsigned_abs() < 9_007_199_254_740_992u128.saturating_mul(1_000_000_000)
}

fn f64_to_i128(value: f64) -> Option<i128> {
    if !value.is_finite() {
        return None;
    }
    let text = format!("{value:.0}");
    text.parse().ok()
}

const DURATION_UNITS: &[(&str, &[&str], &str)] = &[
    ("years", &["long", "short", "narrow"], "short"),
    ("months", &["long", "short", "narrow"], "short"),
    ("weeks", &["long", "short", "narrow"], "short"),
    ("days", &["long", "short", "narrow"], "short"),
    (
        "hours",
        &["long", "short", "narrow", "numeric", "2-digit"],
        "numeric",
    ),
    (
        "minutes",
        &["long", "short", "narrow", "numeric", "2-digit"],
        "2-digit",
    ),
    (
        "seconds",
        &["long", "short", "narrow", "numeric", "2-digit"],
        "2-digit",
    ),
    (
        "milliseconds",
        &["long", "short", "narrow", "numeric"],
        "numeric",
    ),
    (
        "microseconds",
        &["long", "short", "narrow", "numeric"],
        "numeric",
    ),
    (
        "nanoseconds",
        &["long", "short", "narrow", "numeric"],
        "numeric",
    ),
];

fn read_duration_units(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
    base_style: &str,
) -> Result<Vec<(String, DurationUnitSlot)>, i64> {
    let mut units = Vec::with_capacity(DURATION_UNITS.len());
    let mut prev_style = String::new();
    for (name, values, digital_base) in DURATION_UNITS {
        let explicit = get_option_string(ctx, state, options, name, values, None)?;
        let (mut style, mut display_default) = duration_unit_style(
            name,
            explicit.as_deref(),
            base_style,
            digital_base,
            &prev_style,
        );
        if style == "numeric" && matches!(*name, "milliseconds" | "microseconds" | "nanoseconds") {
            style = "fractional".into();
            display_default = "auto";
        }
        let display = get_option_string(
            ctx,
            state,
            options,
            &format!("{name}Display"),
            &["auto", "always"],
            Some(display_default),
        )?
        .unwrap_or_else(|| display_default.to_owned());
        validate_duration_unit_style(ctx, state, name, &style, &display, &prev_style)?;
        if matches!(*name, "minutes" | "seconds")
            && matches!(prev_style.as_str(), "numeric" | "2-digit")
        {
            style = "2-digit".into();
        }
        if matches!(
            *name,
            "hours" | "minutes" | "seconds" | "milliseconds" | "microseconds"
        ) {
            prev_style = style.clone();
        }
        units.push(((*name).to_owned(), DurationUnitSlot { style, display }));
    }
    Ok(units)
}

fn duration_unit_style(
    name: &str,
    explicit: Option<&str>,
    base_style: &str,
    digital_base: &str,
    prev_style: &str,
) -> (String, &'static str) {
    if let Some(style) = explicit {
        return (style.to_owned(), "always");
    }
    if base_style == "digital" {
        let display = if matches!(name, "hours" | "minutes" | "seconds") {
            "always"
        } else {
            "auto"
        };
        return (digital_base.to_owned(), display);
    }
    if matches!(prev_style, "fractional" | "numeric" | "2-digit") {
        let display = if matches!(name, "minutes" | "seconds") {
            "always"
        } else {
            "auto"
        };
        return ("numeric".into(), display);
    }
    (base_style.to_owned(), "auto")
}

fn validate_duration_unit_style(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    name: &str,
    style: &str,
    display: &str,
    prev_style: &str,
) -> Result<(), i64> {
    if display == "always" && style == "fractional" {
        return Err(range_error(
            ctx,
            state,
            &format!("{name} cannot use fractional style with display always"),
        ));
    }
    if prev_style == "fractional" && style != "fractional" {
        return Err(range_error(
            ctx,
            state,
            "fractional duration units must be followed by fractional units",
        ));
    }
    if matches!(prev_style, "numeric" | "2-digit")
        && !matches!(style, "fractional" | "numeric" | "2-digit")
    {
        return Err(range_error(
            ctx,
            state,
            "numeric duration units cannot be followed by long/short/narrow",
        ));
    }
    Ok(())
}
