use chrono::{Datelike, Local, SecondsFormat, Timelike, Utc};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{date, fail_dispatch, modules};
use crate::{NativeAgentState, NativeCallableKind};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DateMethodKind {
    GetDate,
    GetDay,
    GetFullYear,
    GetHours,
    GetMilliseconds,
    GetMinutes,
    GetMonth,
    GetSeconds,
    GetTime,
    GetTimezoneOffset,
    GetUTCDate,
    GetUTCDay,
    GetUTCFullYear,
    GetUTCHours,
    GetUTCMilliseconds,
    GetUTCMinutes,
    GetUTCMonth,
    GetUTCSeconds,
    SetDate,
    SetFullYear,
    SetHours,
    SetMilliseconds,
    SetMinutes,
    SetMonth,
    SetSeconds,
    SetTime,
    SetUTCDate,
    SetUTCFullYear,
    SetUTCHours,
    SetUTCMilliseconds,
    SetUTCMinutes,
    SetUTCMonth,
    SetUTCSeconds,
    ToString,
    ToDateString,
    ToTimeString,
    ToLocaleString,
    ToLocaleDateString,
    ToLocaleTimeString,
    ToISOString,
    ToUTCString,
    ToJSON,
    ValueOf,
}

const DATE_METHODS: &[(&str, DateMethodKind)] = &[
    ("getDate", DateMethodKind::GetDate),
    ("getDay", DateMethodKind::GetDay),
    ("getFullYear", DateMethodKind::GetFullYear),
    ("getHours", DateMethodKind::GetHours),
    ("getMilliseconds", DateMethodKind::GetMilliseconds),
    ("getMinutes", DateMethodKind::GetMinutes),
    ("getMonth", DateMethodKind::GetMonth),
    ("getSeconds", DateMethodKind::GetSeconds),
    ("getTime", DateMethodKind::GetTime),
    ("getTimezoneOffset", DateMethodKind::GetTimezoneOffset),
    ("getUTCDate", DateMethodKind::GetUTCDate),
    ("getUTCDay", DateMethodKind::GetUTCDay),
    ("getUTCFullYear", DateMethodKind::GetUTCFullYear),
    ("getUTCHours", DateMethodKind::GetUTCHours),
    ("getUTCMilliseconds", DateMethodKind::GetUTCMilliseconds),
    ("getUTCMinutes", DateMethodKind::GetUTCMinutes),
    ("getUTCMonth", DateMethodKind::GetUTCMonth),
    ("getUTCSeconds", DateMethodKind::GetUTCSeconds),
    ("setDate", DateMethodKind::SetDate),
    ("setFullYear", DateMethodKind::SetFullYear),
    ("setHours", DateMethodKind::SetHours),
    ("setMilliseconds", DateMethodKind::SetMilliseconds),
    ("setMinutes", DateMethodKind::SetMinutes),
    ("setMonth", DateMethodKind::SetMonth),
    ("setSeconds", DateMethodKind::SetSeconds),
    ("setTime", DateMethodKind::SetTime),
    ("setUTCDate", DateMethodKind::SetUTCDate),
    ("setUTCFullYear", DateMethodKind::SetUTCFullYear),
    ("setUTCHours", DateMethodKind::SetUTCHours),
    ("setUTCMilliseconds", DateMethodKind::SetUTCMilliseconds),
    ("setUTCMinutes", DateMethodKind::SetUTCMinutes),
    ("setUTCMonth", DateMethodKind::SetUTCMonth),
    ("setUTCSeconds", DateMethodKind::SetUTCSeconds),
    ("toString", DateMethodKind::ToString),
    ("toDateString", DateMethodKind::ToDateString),
    ("toTimeString", DateMethodKind::ToTimeString),
    ("toLocaleString", DateMethodKind::ToLocaleString),
    ("toLocaleDateString", DateMethodKind::ToLocaleDateString),
    ("toLocaleTimeString", DateMethodKind::ToLocaleTimeString),
    ("toISOString", DateMethodKind::ToISOString),
    ("toUTCString", DateMethodKind::ToUTCString),
    ("toJSON", DateMethodKind::ToJSON),
    ("valueOf", DateMethodKind::ValueOf),
];

pub(super) fn install_prototype_methods(
    state: &mut NativeAgentState,
    prototype: i64,
) -> Result<(), ()> {
    for &(name, kind) in DATE_METHODS {
        let key = state
            .intern_text(name.into(), value::TAG_STRING)
            .ok_or(())?;
        let callable = state
            .native_callable(NativeCallableKind::DateMethod(kind))
            .ok_or(())?;
        state
            .gc
            .heap()
            .set_property(
                value::decode_handle(prototype),
                value::decode_handle(key),
                callable as u64,
            )
            .map_err(|_| ())?;
    }
    Ok(())
}

pub(crate) fn method(
    state: &mut NativeAgentState,
    receiver: i64,
    key: &str,
) -> Option<DateMethodKind> {
    date::parts(state, receiver)?;
    DATE_METHODS
        .iter()
        .find_map(|(name, kind)| (*name == key).then_some(*kind))
}

pub(crate) fn call_method(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    kind: DateMethodKind,
    args: &[i64],
) -> i64 {
    let Some((milliseconds, _)) = date::parts(state, receiver) else {
        return exception(
            ctx,
            state,
            "TypeError",
            "Date method called on incompatible receiver",
        );
    };
    match kind {
        DateMethodKind::GetTime | DateMethodKind::ValueOf => value::encode_f64(milliseconds),
        DateMethodKind::GetDate => local_component(milliseconds, |date| f64::from(date.day())),
        DateMethodKind::GetDay => local_component(milliseconds, |date| {
            f64::from(date.weekday().num_days_from_sunday())
        }),
        DateMethodKind::GetFullYear => local_component(milliseconds, |date| f64::from(date.year())),
        DateMethodKind::GetHours => local_component(milliseconds, |date| f64::from(date.hour())),
        DateMethodKind::GetMilliseconds => local_component(milliseconds, |date| {
            f64::from(date.nanosecond() / 1_000_000)
        }),
        DateMethodKind::GetMinutes => {
            local_component(milliseconds, |date| f64::from(date.minute()))
        }
        DateMethodKind::GetMonth => local_component(milliseconds, |date| f64::from(date.month0())),
        DateMethodKind::GetSeconds => {
            local_component(milliseconds, |date| f64::from(date.second()))
        }
        DateMethodKind::GetTimezoneOffset => local_timezone_offset(milliseconds),
        DateMethodKind::GetUTCDate => utc_component(milliseconds, |date| f64::from(date.day())),
        DateMethodKind::GetUTCDay => utc_component(milliseconds, |date| {
            f64::from(date.weekday().num_days_from_sunday())
        }),
        DateMethodKind::GetUTCFullYear => {
            utc_component(milliseconds, |date| f64::from(date.year()))
        }
        DateMethodKind::GetUTCHours => utc_component(milliseconds, |date| f64::from(date.hour())),
        DateMethodKind::GetUTCMilliseconds => utc_component(milliseconds, |date| {
            f64::from(date.nanosecond() / 1_000_000)
        }),
        DateMethodKind::GetUTCMinutes => {
            utc_component(milliseconds, |date| f64::from(date.minute()))
        }
        DateMethodKind::GetUTCMonth => utc_component(milliseconds, |date| f64::from(date.month0())),
        DateMethodKind::GetUTCSeconds => {
            utc_component(milliseconds, |date| f64::from(date.second()))
        }
        DateMethodKind::ToString => string_value(ctx, state, date::render_local_date(milliseconds)),
        DateMethodKind::ToDateString => string_value(ctx, state, format_local_date(milliseconds)),
        DateMethodKind::ToTimeString => string_value(ctx, state, format_local_time(milliseconds)),
        DateMethodKind::ToLocaleString => {
            string_value(ctx, state, format_local_datetime(milliseconds))
        }
        DateMethodKind::ToLocaleDateString => {
            string_value(ctx, state, format_local_date(milliseconds))
        }
        DateMethodKind::ToLocaleTimeString => {
            string_value(ctx, state, format_local_time(milliseconds))
        }
        DateMethodKind::ToISOString => to_iso_string(ctx, state, milliseconds),
        DateMethodKind::ToUTCString => to_utc_string(ctx, state, milliseconds),
        DateMethodKind::ToJSON => {
            if wjsm_builtins::ms_to_datetime_utc(milliseconds).is_some() {
                to_iso_string(ctx, state, milliseconds)
            } else {
                value::encode_null()
            }
        }
        DateMethodKind::SetDate
        | DateMethodKind::SetFullYear
        | DateMethodKind::SetHours
        | DateMethodKind::SetMilliseconds
        | DateMethodKind::SetMinutes
        | DateMethodKind::SetMonth
        | DateMethodKind::SetSeconds
        | DateMethodKind::SetTime
        | DateMethodKind::SetUTCDate
        | DateMethodKind::SetUTCFullYear
        | DateMethodKind::SetUTCHours
        | DateMethodKind::SetUTCMilliseconds
        | DateMethodKind::SetUTCMinutes
        | DateMethodKind::SetUTCMonth
        | DateMethodKind::SetUTCSeconds => {
            set_method(ctx, state, receiver, kind, milliseconds, args)
        }
    }
}

fn set_method(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    kind: DateMethodKind,
    milliseconds: f64,
    args: &[i64],
) -> i64 {
    if kind == DateMethodKind::SetTime {
        let milliseconds = args
            .first()
            .and_then(|argument| super::runtime::to_number(state, *argument))
            .map_or(f64::NAN, date::time_clip);
        return store_millis(ctx, state, receiver, milliseconds);
    }
    let is_utc = matches!(
        kind,
        DateMethodKind::SetUTCDate
            | DateMethodKind::SetUTCFullYear
            | DateMethodKind::SetUTCHours
            | DateMethodKind::SetUTCMilliseconds
            | DateMethodKind::SetUTCMinutes
            | DateMethodKind::SetUTCMonth
            | DateMethodKind::SetUTCSeconds
    );
    let full_year = matches!(
        kind,
        DateMethodKind::SetFullYear | DateMethodKind::SetUTCFullYear
    );
    let fields = match date::date_fields(milliseconds, is_utc) {
        Some(fields) => fields,
        None if full_year => date::DateFields {
            year: 0,
            month: 0,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            millisecond: 0,
        },
        None => return store_millis(ctx, state, receiver, f64::NAN),
    };
    let milliseconds = update_fields(state, kind, fields, args)
        .and_then(|fields| date::make_date_ms(fields, is_utc))
        .map_or(f64::NAN, date::time_clip);
    store_millis(ctx, state, receiver, milliseconds)
}

fn update_fields(
    state: &NativeAgentState,
    kind: DateMethodKind,
    mut fields: date::DateFields,
    args: &[i64],
) -> Option<date::DateFields> {
    match kind {
        DateMethodKind::SetDate | DateMethodKind::SetUTCDate => {
            fields.day = required_argument(state, args, 0)?;
        }
        DateMethodKind::SetFullYear | DateMethodKind::SetUTCFullYear => {
            fields.year = required_argument(state, args, 0)?;
            fields.month = optional_argument(state, args, 1, fields.month)?;
            fields.day = optional_argument(state, args, 2, fields.day)?;
        }
        DateMethodKind::SetHours | DateMethodKind::SetUTCHours => {
            fields.hour = required_argument(state, args, 0)?;
            fields.minute = optional_argument(state, args, 1, fields.minute)?;
            fields.second = optional_argument(state, args, 2, fields.second)?;
            fields.millisecond = optional_argument(state, args, 3, fields.millisecond)?;
        }
        DateMethodKind::SetMilliseconds | DateMethodKind::SetUTCMilliseconds => {
            fields.millisecond = required_argument(state, args, 0)?;
        }
        DateMethodKind::SetMinutes | DateMethodKind::SetUTCMinutes => {
            fields.minute = required_argument(state, args, 0)?;
            fields.second = optional_argument(state, args, 1, fields.second)?;
            fields.millisecond = optional_argument(state, args, 2, fields.millisecond)?;
        }
        DateMethodKind::SetMonth | DateMethodKind::SetUTCMonth => {
            fields.month = required_argument(state, args, 0)?;
            fields.day = optional_argument(state, args, 1, fields.day)?;
        }
        DateMethodKind::SetSeconds | DateMethodKind::SetUTCSeconds => {
            fields.second = required_argument(state, args, 0)?;
            fields.millisecond = optional_argument(state, args, 1, fields.millisecond)?;
        }
        _ => return None,
    }
    Some(fields)
}

fn store_millis(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    milliseconds: f64,
) -> i64 {
    if modules::set_named_property(
        state,
        receiver,
        "__date_ms__",
        value::encode_f64(milliseconds),
    )
    .is_err()
    {
        return fail_dispatch(ctx);
    }
    value::encode_f64(milliseconds)
}

fn required_argument(state: &NativeAgentState, args: &[i64], index: usize) -> Option<i64> {
    integer_argument(state, args.get(index).copied())
}

fn optional_argument(
    state: &NativeAgentState,
    args: &[i64],
    index: usize,
    default: i64,
) -> Option<i64> {
    args.get(index).map_or(Some(default), |argument| {
        integer_argument(state, Some(*argument))
    })
}

fn integer_argument(state: &NativeAgentState, argument: Option<i64>) -> Option<i64> {
    let number = super::runtime::to_number(state, argument?)?;
    if !number.is_finite() || number < i64::MIN as f64 || number > i64::MAX as f64 {
        return None;
    }
    Some(number.trunc() as i64)
}

fn local_component(
    milliseconds: f64,
    component: impl FnOnce(chrono::DateTime<Local>) -> f64,
) -> i64 {
    value::encode_f64(wjsm_builtins::ms_to_datetime_local(milliseconds).map_or(f64::NAN, component))
}

fn utc_component(milliseconds: f64, component: impl FnOnce(chrono::DateTime<Utc>) -> f64) -> i64 {
    value::encode_f64(wjsm_builtins::ms_to_datetime_utc(milliseconds).map_or(f64::NAN, component))
}

fn local_timezone_offset(milliseconds: f64) -> i64 {
    let offset = wjsm_builtins::ms_to_datetime_local(milliseconds).map_or(f64::NAN, |date| {
        -f64::from(date.offset().local_minus_utc()) / 60.0
    });
    value::encode_f64(offset)
}

fn to_iso_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    milliseconds: f64,
) -> i64 {
    let Some(date) = wjsm_builtins::ms_to_datetime_utc(milliseconds) else {
        return range_error(ctx, state, "Invalid time value");
    };
    string_value(
        ctx,
        state,
        date.to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

fn to_utc_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    milliseconds: f64,
) -> i64 {
    let text = wjsm_builtins::ms_to_datetime_utc(milliseconds).map_or_else(
        || "Invalid Date".to_owned(),
        |date| date.format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
    );
    string_value(ctx, state, text)
}

fn range_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    exception(ctx, state, "RangeError", message)
}

fn exception(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    name: &str,
    message: &str,
) -> i64 {
    modules::named_error_object(state, name, message.to_owned())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn string_value(ctx: &mut NativeVmContext, state: &mut NativeAgentState, text: String) -> i64 {
    state
        .intern_text(text, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn format_local_date(milliseconds: f64) -> String {
    wjsm_builtins::ms_to_datetime_local(milliseconds).map_or_else(
        || "Invalid Date".to_owned(),
        |date| date.format("%a %b %e %Y").to_string(),
    )
}

fn format_local_time(milliseconds: f64) -> String {
    wjsm_builtins::ms_to_datetime_local(milliseconds).map_or_else(
        || "Invalid Date".to_owned(),
        |date| date.format("%H:%M:%S GMT%:z").to_string(),
    )
}

fn format_local_datetime(milliseconds: f64) -> String {
    wjsm_builtins::ms_to_datetime_local(milliseconds).map_or_else(
        || "Invalid Date".to_owned(),
        |date| date.format("%a %b %e %Y %H:%M:%S").to_string(),
    )
}
