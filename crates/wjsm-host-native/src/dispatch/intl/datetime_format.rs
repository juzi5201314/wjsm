//! `Intl.DateTimeFormat`。

use std::collections::BTreeMap;

use wjsm_builtins::intl::resolve_locale_filtered;
use wjsm_intl_data::{
    DateTimeFormatSpec, OwnedDateTimeFormatter, available_calendars, available_numbering_systems,
    canonicalize_time_zone, canonicalize_unicode_keyword,
};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::common::{
    create_instance, intern, parts_array, resolved_object, slot_handle, throw_intl,
};
use super::js::{
    canonicalize_locales, get_number_option, get_option_bool_opt, get_option_string,
    get_options_object, supported_locales_of, to_object,
};
use super::slots::{DateTimeFormatSlot, IntlSlot};
use super::{IntlCallable, incompatible};
use crate::NativeAgentState;
use crate::NativeCallableKind;
use crate::dispatch::date;
use crate::dispatch::runtime::{fail_dispatch, range_error, to_number_coerced, type_error};

#[derive(Clone, Copy)]
pub(super) enum DateTimeDefaults {
    Date,
    Time,
    All,
}

#[derive(Clone, Copy)]
pub(super) enum DateTimeRequired {
    Any,
    Date,
    Time,
}

pub(super) fn format_with_options(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    locales: i64,
    options: i64,
    value: i64,
    required: DateTimeRequired,
    defaults: DateTimeDefaults,
) -> i64 {
    let instance = construct_with_defaults(
        ctx,
        state,
        value::encode_undefined(),
        &[locales, options],
        required,
        defaults,
    );
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
        IntlCallable::DateTimeFormatConstructor => construct_with_defaults(
            ctx,
            state,
            receiver,
            args,
            DateTimeRequired::Any,
            DateTimeDefaults::Date,
        ),
        IntlCallable::DateTimeFormatSupportedLocalesOf => supported_locales_of(ctx, state, args),
        IntlCallable::DateTimeFormatResolvedOptions => resolved(ctx, state, receiver),
        IntlCallable::DateTimeFormatFormatGet => format_get(ctx, state, receiver),
        IntlCallable::DateTimeFormatFormat(handle) => format_value(ctx, state, handle, args, false),
        IntlCallable::DateTimeFormatFormatToParts => match slot_handle(receiver) {
            Some(handle) => format_value(ctx, state, handle, args, true),
            None => incompatible(ctx, state),
        },
        IntlCallable::DateTimeFormatFormatRange => format_range(ctx, state, receiver, args, false),
        IntlCallable::DateTimeFormatFormatRangeToParts => {
            format_range(ctx, state, receiver, args, true)
        }
        _ => fail_dispatch(ctx),
    }
}

fn construct_with_defaults(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
    required: DateTimeRequired,
    defaults: DateTimeDefaults,
) -> i64 {
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
    match read_options(ctx, state, &requested, options, required, defaults) {
        Ok(slot) => create_instance(
            ctx,
            state,
            IntlCallable::DateTimeFormatConstructor,
            IntlSlot::DateTimeFormat(slot),
            this_value,
        ),
        Err(exception) => exception,
    }
}

fn read_options(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    requested: &[String],
    options: i64,
    required: DateTimeRequired,
    defaults: DateTimeDefaults,
) -> Result<DateTimeFormatSlot, i64> {
    let _ = get_option_string(
        ctx,
        state,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    )?;
    let calendar = super::js::require_unicode_type(ctx, state, options, "calendar")?
        .map(|value| canonicalize_unicode_keyword("ca", &value.to_ascii_lowercase()));
    let numbering_system = super::js::require_unicode_type(ctx, state, options, "numberingSystem")?;
    let hour12 = get_option_bool_opt(ctx, state, options, "hour12")?;
    let mut hour_cycle = get_option_string(
        ctx,
        state,
        options,
        "hourCycle",
        &["h11", "h12", "h23", "h24"],
        None,
    )?;
    let time_zone = match get_option_string(ctx, state, options, "timeZone", &[], None)? {
        Some(name) => {
            Some(canonicalize_time_zone(&name).map_err(|error| range_error(ctx, state, &error))?)
        }
        None => None,
    };
    let weekday = named_style_field(ctx, state, options, "weekday")?;
    let era = named_style_field(ctx, state, options, "era")?;
    let year = field(ctx, state, options, "year")?;
    let month = month_field(ctx, state, options)?;
    let day = field(ctx, state, options, "day")?;
    let day_period = get_option_string(
        ctx,
        state,
        options,
        "dayPeriod",
        &["narrow", "short", "long"],
        None,
    )?;
    let hour = field(ctx, state, options, "hour")?;
    let minute = field(ctx, state, options, "minute")?;
    let second = field(ctx, state, options, "second")?;
    let fractional = get_number_option(
        ctx,
        state,
        options,
        "fractionalSecondDigits",
        1.0,
        3.0,
        None,
    )?
    .map(|value| value as u32);
    let time_zone_name = get_option_string(
        ctx,
        state,
        options,
        "timeZoneName",
        &[
            "short",
            "long",
            "shortOffset",
            "longOffset",
            "shortGeneric",
            "longGeneric",
        ],
        None,
    )?;
    let _ = get_option_string(
        ctx,
        state,
        options,
        "formatMatcher",
        &["basic", "best fit"],
        Some("best fit"),
    )?;
    let date_style = get_option_string(
        ctx,
        state,
        options,
        "dateStyle",
        &["full", "long", "medium", "short"],
        None,
    )?;
    let time_style = get_option_string(
        ctx,
        state,
        options,
        "timeStyle",
        &["full", "long", "medium", "short"],
        None,
    )?;
    let has_style = date_style.is_some() || time_style.is_some();
    let has_date_fields = weekday.is_some() || year.is_some() || month.is_some() || day.is_some();
    let has_time_fields = day_period.is_some()
        || hour.is_some()
        || minute.is_some()
        || second.is_some()
        || fractional.is_some();
    let has_fields =
        has_date_fields || has_time_fields || era.is_some() || time_zone_name.is_some();
    if has_style && has_fields {
        return Err(type_error(
            ctx,
            state,
            "dateStyle/timeStyle cannot be used with date-time fields",
        ));
    }
    let (year, month, day, hour, minute, second) = apply_to_date_time_options(
        required,
        defaults,
        has_style,
        has_date_fields,
        has_time_fields,
        year,
        month,
        day,
        hour,
        minute,
        second,
    );
    let hour12_present = hour12.is_some();
    let mut ext = BTreeMap::new();
    if let Some(calendar) = calendar
        .as_deref()
        .filter(|calendar| available_calendars().contains(calendar))
    {
        ext.insert("ca".into(), calendar.to_owned());
    }
    if let Some(system) = numbering_system
        .as_deref()
        .filter(|system| available_numbering_systems().contains(system))
    {
        ext.insert("nu".into(), system.to_owned());
    }
    if !hour12_present && let Some(cycle) = &hour_cycle {
        ext.insert("hc".into(), cycle.clone());
    }
    let mut resolved = resolve_locale_filtered(
        requested,
        &["ca", "nu", "hc"],
        &ext,
        |key, value, _| match key {
            "ca" => available_calendars().contains(&value),
            "nu" => available_numbering_systems().contains(&value),
            "hc" => matches!(value, "h11" | "h12" | "h23" | "h24"),
            _ => false,
        },
    )
    .map_err(|error| throw_intl(ctx, state, error))?;
    if hour12_present {
        resolved.locale = strip_unicode_key(&resolved.locale, "hc");
    }
    hour_cycle = match hour12 {
        Some(true) => Some(hour_cycle_12(&resolved.locale).into()),
        Some(false) => Some("h23".into()),
        None => hour_cycle.or_else(|| resolved.extensions.get("hc").cloned()),
    };
    if hour.is_none() && time_style.is_none() {
        hour_cycle = None;
    } else if hour_cycle.is_none() {
        hour_cycle = Some(default_hour_cycle(&resolved.locale).into());
    }
    Ok(DateTimeFormatSlot {
        locale: resolved.locale,
        calendar: resolved
            .extensions
            .get("ca")
            .cloned()
            .unwrap_or_else(|| "gregory".into()),
        numbering_system: resolved
            .extensions
            .get("nu")
            .cloned()
            .unwrap_or_else(|| "latn".into()),
        time_zone: time_zone.clone().unwrap_or_else(|| "UTC".into()),
        implicit_local: time_zone.is_none(),
        hour_cycle,
        date_style,
        time_style,
        weekday,
        era,
        year,
        month,
        day,
        day_period,
        hour,
        minute,
        second,
        fractional_second_digits: fractional,
        time_zone_name,
        bound_format: None,
        formatter: None,
    })
}

fn field(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
    name: &str,
) -> Result<Option<String>, i64> {
    get_option_string(ctx, state, options, name, &["numeric", "2-digit"], None)
}

fn named_style_field(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
    name: &str,
) -> Result<Option<String>, i64> {
    get_option_string(
        ctx,
        state,
        options,
        name,
        &["narrow", "short", "long"],
        None,
    )
}

fn month_field(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
) -> Result<Option<String>, i64> {
    get_option_string(
        ctx,
        state,
        options,
        "month",
        &["numeric", "2-digit", "narrow", "short", "long"],
        None,
    )
}

fn resolved(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::DateTimeFormat(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let locale = slot.locale.clone();
    let calendar = slot.calendar.clone();
    let numbering_system = slot.numbering_system.clone();
    let time_zone = slot.time_zone.clone();
    let hour_cycle = slot.hour_cycle.clone();
    let date_style = slot.date_style.clone();
    let time_style = slot.time_style.clone();
    let weekday = slot.weekday.clone();
    let era = slot.era.clone();
    let year = slot.year.clone();
    let month = slot.month.clone();
    let day = slot.day.clone();
    let day_period = slot.day_period.clone();
    let hour = slot.hour.clone();
    let minute = slot.minute.clone();
    let second = slot.second.clone();
    let time_zone_name = slot.time_zone_name.clone();
    let digits = slot.fractional_second_digits;
    let hour12 = hour_cycle
        .as_deref()
        .map(|cycle| cycle == "h11" || cycle == "h12");
    let show_hour12 = hour.is_some() || time_style.is_some();
    let locale = intern(ctx, state, locale);
    let calendar = intern(ctx, state, calendar);
    let numbering_system = intern(ctx, state, numbering_system);
    let time_zone = intern(ctx, state, time_zone);
    let mut fields = vec![
        ("locale", locale),
        ("calendar", calendar),
        ("numberingSystem", numbering_system),
        ("timeZone", time_zone),
    ];
    // ECMA-402：timeZone → hourCycle → hour12 → weekday …
    if let Some(cycle) = hour_cycle {
        fields.push(("hourCycle", intern(ctx, state, cycle)));
    }
    if show_hour12 && let Some(hour12) = hour12 {
        fields.push(("hour12", value::encode_bool(hour12)));
    }
    for (name, stored) in [
        ("weekday", weekday),
        ("era", era),
        ("year", year),
        ("month", month),
        ("day", day),
        ("dayPeriod", day_period),
        ("hour", hour),
        ("minute", minute),
        ("second", second),
    ] {
        if let Some(stored) = stored {
            fields.push((name, intern(ctx, state, stored)));
        }
    }
    if let Some(digits) = digits {
        fields.push(("fractionalSecondDigits", value::encode_f64(digits as f64)));
    }
    if let Some(name) = time_zone_name {
        fields.push(("timeZoneName", intern(ctx, state, name)));
    }
    if let Some(style) = date_style {
        fields.push(("dateStyle", intern(ctx, state, style)));
    }
    if let Some(style) = time_style {
        fields.push(("timeStyle", intern(ctx, state, style)));
    }
    resolved_object(ctx, state, &fields)
}

fn format_get(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::DateTimeFormat(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    if let Some(bound) = slot.bound_format {
        return bound;
    }
    let Some(bound) = state.native_callable(NativeCallableKind::Intl(
        IntlCallable::DateTimeFormatFormat(handle),
    )) else {
        return fail_dispatch(ctx);
    };
    if let Some(IntlSlot::DateTimeFormat(slot)) = state.intl.slots.get_mut(&handle) {
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
    let millis = match date_millis(ctx, state, args.first().copied(), false) {
        Ok(millis) => millis,
        Err(exception) => return exception,
    };
    let millis = date::time_clip(millis);
    if !millis.is_finite() {
        return range_error(ctx, state, "Invalid time value");
    }
    if let Err(exception) = ensure_formatter(ctx, state, handle) {
        return exception;
    }
    let Some(IntlSlot::DateTimeFormat(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let millis = zone_millis(millis, &slot.time_zone, slot.implicit_local);
    let Some(formatter) = slot.formatter.as_ref() else {
        return fail_dispatch(ctx);
    };
    if parts {
        return match formatter.format_parts_millis(millis) {
            Ok(parts) => parts_array(ctx, state, parts),
            Err(error) => range_error(ctx, state, &error),
        };
    }
    match formatter.format_millis(millis) {
        Ok(text) => intern(ctx, state, text),
        Err(error) => range_error(ctx, state, &error),
    }
}

fn wall_millis(millis: f64) -> f64 {
    use chrono::{Datelike, TimeZone, Timelike, Utc};
    let Some(local) = chrono::Local.timestamp_millis_opt(millis as i64).single() else {
        return millis;
    };
    let millis_of_second = f64::from(local.nanosecond() / 1_000_000);
    Utc.with_ymd_and_hms(
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        local.second(),
    )
    .single()
    .map(|utc| utc.timestamp_millis() as f64 + millis_of_second)
    .unwrap_or(millis)
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
    let start_arg = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let end_arg = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if value::is_undefined(start_arg) || value::is_undefined(end_arg) {
        return type_error(ctx, state, "formatRange requires two dates");
    }
    let start_ms = match date_millis(ctx, state, Some(start_arg), true) {
        Ok(millis) => date::time_clip(millis),
        Err(exception) => return exception,
    };
    let end_ms = match date_millis(ctx, state, Some(end_arg), true) {
        Ok(millis) => date::time_clip(millis),
        Err(exception) => return exception,
    };
    if !start_ms.is_finite() || !end_ms.is_finite() {
        return range_error(ctx, state, "Invalid time value");
    }
    if let Err(exception) = ensure_formatter(ctx, state, handle) {
        return exception;
    }
    let Some(IntlSlot::DateTimeFormat(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let start = zone_millis(start_ms, &slot.time_zone, slot.implicit_local);
    let end = zone_millis(end_ms, &slot.time_zone, slot.implicit_local);
    let Some(formatter) = slot.formatter.as_ref() else {
        return fail_dispatch(ctx);
    };
    if parts {
        return match formatter.format_range_parts_millis(start, end) {
            Ok(range_parts) => parts_array(ctx, state, range_parts),
            Err(error) => range_error(ctx, state, &error),
        };
    }
    match formatter.format_range_millis(start, end) {
        Ok(text) => intern(ctx, state, text),
        Err(error) => range_error(ctx, state, &error),
    }
}

fn date_millis(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    input: Option<i64>,
    required: bool,
) -> Result<f64, i64> {
    let Some(input) = input.filter(|value| !value::is_undefined(*value)) else {
        if required {
            return Err(type_error(ctx, state, "date is undefined"));
        }
        return Ok(chrono::Utc::now().timestamp_millis() as f64);
    };
    if let Some((millis, _)) = date::parts(state, input) {
        return Ok(millis);
    }
    to_number_coerced(ctx, state, input)
}

fn apply_to_date_time_options(
    required: DateTimeRequired,
    defaults: DateTimeDefaults,
    has_style: bool,
    has_date_fields: bool,
    has_time_fields: bool,
    year: Option<String>,
    month: Option<String>,
    day: Option<String>,
    hour: Option<String>,
    minute: Option<String>,
    second: Option<String>,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let date_counts = matches!(required, DateTimeRequired::Date | DateTimeRequired::Any);
    let time_counts = matches!(required, DateTimeRequired::Time | DateTimeRequired::Any);
    let need_defaults =
        !has_style && !(date_counts && has_date_fields) && !(time_counts && has_time_fields);
    if !need_defaults {
        return (year, month, day, hour, minute, second);
    }
    let date = matches!(defaults, DateTimeDefaults::Date | DateTimeDefaults::All);
    let time = matches!(defaults, DateTimeDefaults::Time | DateTimeDefaults::All);
    (
        if date { Some("numeric".into()) } else { year },
        if date { Some("numeric".into()) } else { month },
        if date { Some("numeric".into()) } else { day },
        if time { Some("numeric".into()) } else { hour },
        if time { Some("numeric".into()) } else { minute },
        if time { Some("numeric".into()) } else { second },
    )
}

fn strip_unicode_key(tag: &str, key: &str) -> String {
    let Some((base, rest)) = tag.split_once("-u-") else {
        return tag.to_owned();
    };
    let mut parts = rest.split('-').peekable();
    let mut kept = Vec::new();
    while let Some(part) = parts.next() {
        if part == key {
            if parts.peek().is_some_and(|next| next.len() != 2) {
                parts.next();
            }
            continue;
        }
        kept.push(part);
    }
    if kept.is_empty() {
        base.to_owned()
    } else {
        format!("{base}-u-{}", kept.join("-"))
    }
}

fn zone_millis(millis: f64, time_zone: &str, implicit_local: bool) -> f64 {
    if implicit_local {
        return wall_millis(millis);
    }
    if let Some(minutes) = offset_minutes(time_zone) {
        return millis + f64::from(minutes) * 60_000.0;
    }
    millis
}

fn offset_minutes(time_zone: &str) -> Option<i32> {
    if let Some(rest) = time_zone
        .strip_prefix("Etc/GMT")
        .or_else(|| time_zone.strip_prefix("etc/gmt"))
    {
        if rest.is_empty() {
            return Some(0);
        }
        let hours: i32 = rest.parse().ok()?;
        return hours.checked_mul(-60);
    }
    let rest = time_zone
        .strip_prefix('+')
        .or_else(|| time_zone.strip_prefix('-'))?;
    let (hours, minutes) = rest.split_once(':')?;
    let hours: i32 = hours.parse().ok()?;
    let minutes: i32 = minutes.parse().ok()?;
    let total = hours.checked_mul(60)?.checked_add(minutes)?;
    Some(if time_zone.starts_with('-') {
        -total
    } else {
        total
    })
}

fn default_hour_cycle(locale: &str) -> &'static str {
    match locale.split(['-', '_']).next().unwrap_or(locale) {
        "en" | "es" | "ar" | "zh" | "ko" | "fil" | "hi" => "h12",
        _ => "h23",
    }
}

fn hour_cycle_12(locale: &str) -> &'static str {
    match locale.split(['-', '_']).next().unwrap_or(locale) {
        "ja" => "h11",
        _ => "h12",
    }
}

fn ensure_formatter(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
) -> Result<(), i64> {
    let Some(IntlSlot::DateTimeFormat(slot)) = state.intl.slots.get(&handle) else {
        return Err(incompatible(ctx, state));
    };
    if slot.formatter.is_some() {
        return Ok(());
    }
    let spec = DateTimeFormatSpec {
        locale: slot.locale.clone(),
        calendar: slot.calendar.clone(),
        numbering_system: slot.numbering_system.clone(),
        hour_cycle: slot.hour_cycle.clone(),
        date_style: slot.date_style.clone(),
        time_style: slot.time_style.clone(),
        weekday: slot.weekday.clone(),
        era: slot.era.clone(),
        year: slot.year.clone(),
        month: slot.month.clone(),
        day: slot.day.clone(),
        day_period: slot.day_period.clone(),
        hour: slot.hour.clone(),
        minute: slot.minute.clone(),
        second: slot.second.clone(),
        fractional_second_digits: slot.fractional_second_digits,
        time_zone: slot.time_zone.clone(),
        time_zone_name: slot.time_zone_name.clone(),
    };
    let formatter =
        OwnedDateTimeFormatter::try_new(&spec).map_err(|error| range_error(ctx, state, &error))?;
    if let Some(IntlSlot::DateTimeFormat(slot)) = state.intl.slots.get_mut(&handle) {
        slot.formatter = Some(formatter);
    }
    Ok(())
}
