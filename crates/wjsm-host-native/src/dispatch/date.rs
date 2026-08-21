use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Timelike, Utc};
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::{fail_dispatch, modules};
use crate::NativeAgentState;

pub(crate) use super::date_methods::{DateMethodKind, call_method, method, method_metadata};
pub(crate) fn install_prototype_methods(
    state: &mut NativeAgentState,
    prototype: i64,
) -> Result<(), ()> {
    super::date_methods::install_prototype_methods(state, prototype)
}

pub(super) fn dispatch_date(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    match builtin {
        Builtin::DateConstructor => Some(string_value(
            ctx,
            state,
            render_local_date(unix_time_millis()),
        )),
        Builtin::DateConstructorNew => Some(construct_date(ctx, state, args)),
        Builtin::DateNow => Some(value::encode_f64(unix_time_millis())),
        Builtin::DateParse => Some(parse_date(state, args)),
        Builtin::DateUTC => Some(date_utc(state, args)),
        _ => None,
    }
}

fn construct_date(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let milliseconds = match args {
        [] => unix_time_millis(),
        [argument] => one_argument_millis(state, *argument),
        _ => date_args_to_ms(state, args, false),
    };
    let Some(object) = state.allocate_object(1, false).ok() else {
        return fail_dispatch(ctx);
    };
    if modules::set_named_property(
        state,
        object,
        "__date_ms__",
        value::encode_f64(milliseconds),
    )
    .is_err()
        || set_date_prototype(state, object).is_err()
    {
        return fail_dispatch(ctx);
    }
    object
}

fn one_argument_millis(state: &mut NativeAgentState, argument: i64) -> f64 {
    if value::is_undefined(argument) {
        return unix_time_millis();
    }
    if value::is_f64(argument) {
        return time_clip(value::decode_f64(argument));
    }
    if value::is_string(argument) {
        return state
            .string_owned(argument)
            .and_then(|text| text.to_utf8())
            .and_then(|text| wjsm_builtins::parse_date_string(&text))
            .map_or(f64::NAN, time_clip);
    }
    if value::is_js_object(argument) {
        return parts(state, argument)
            .map(|(milliseconds, _)| milliseconds)
            .unwrap_or(f64::NAN);
    }
    f64::NAN
}

fn parse_date(state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(argument) = args.first().copied() else {
        return value::encode_f64(f64::NAN);
    };
    let milliseconds = state
        .string_owned(argument)
        .and_then(|text| text.to_utf8())
        .and_then(|text| wjsm_builtins::parse_date_string(&text))
        .map_or(f64::NAN, time_clip);
    value::encode_f64(milliseconds)
}

fn date_utc(state: &NativeAgentState, args: &[i64]) -> i64 {
    value::encode_f64(date_args_to_ms(state, args, true))
}

fn date_args_to_ms(state: &NativeAgentState, args: &[i64], is_utc: bool) -> f64 {
    if args.len() < 2 {
        return f64::NAN;
    }
    let Some(year) = integer_argument(state, args.first().copied()) else {
        return f64::NAN;
    };
    let Some(month) = integer_argument(state, args.get(1).copied()) else {
        return f64::NAN;
    };
    let Some(day) = optional_argument(state, args, 2, 1) else {
        return f64::NAN;
    };
    let Some(hour) = optional_argument(state, args, 3, 0) else {
        return f64::NAN;
    };
    let Some(minute) = optional_argument(state, args, 4, 0) else {
        return f64::NAN;
    };
    let Some(second) = optional_argument(state, args, 5, 0) else {
        return f64::NAN;
    };
    let Some(millisecond) = optional_argument(state, args, 6, 0) else {
        return f64::NAN;
    };
    let fields = DateFields {
        year: if (0..=99).contains(&year) {
            year + 1900
        } else {
            year
        },
        month,
        day,
        hour,
        minute,
        second,
        millisecond,
    };
    make_date_ms(fields, is_utc).map_or(f64::NAN, time_clip)
}

fn set_date_prototype(state: &mut NativeAgentState, object: i64) -> Result<(), ()> {
    let global = state.global_object.ok_or(())?;
    let date_key = state
        .intern_text("Date".into(), value::TAG_STRING)
        .ok_or(())?;
    let constructor = state.global_property(global, date_key).ok_or(())?;
    let prototype_key = state.intern_property_string("prototype".into()).ok_or(())?;
    let prototype = state
        .callable_property(constructor, prototype_key)
        .filter(|prototype| value::is_object(*prototype))
        .map(value::decode_handle)
        .ok_or(())?;
    state
        .gc
        .heap()
        .set_prototype(value::decode_handle(object), prototype)
        .map_err(|_| ())
}

#[derive(Clone, Copy)]
pub(super) struct DateFields {
    pub(super) year: i64,
    pub(super) month: i64,
    pub(super) day: i64,
    pub(super) hour: i64,
    pub(super) minute: i64,
    pub(super) second: i64,
    pub(super) millisecond: i64,
}

pub(super) fn date_fields(milliseconds: f64, is_utc: bool) -> Option<DateFields> {
    if is_utc {
        wjsm_builtins::ms_to_datetime_utc(milliseconds).map(|date| DateFields {
            year: i64::from(date.year()),
            month: i64::from(date.month0()),
            day: i64::from(date.day()),
            hour: i64::from(date.hour()),
            minute: i64::from(date.minute()),
            second: i64::from(date.second()),
            millisecond: i64::from(date.nanosecond() / 1_000_000),
        })
    } else {
        wjsm_builtins::ms_to_datetime_local(milliseconds).map(|date| DateFields {
            year: i64::from(date.year()),
            month: i64::from(date.month0()),
            day: i64::from(date.day()),
            hour: i64::from(date.hour()),
            minute: i64::from(date.minute()),
            second: i64::from(date.second()),
            millisecond: i64::from(date.nanosecond() / 1_000_000),
        })
    }
}

pub(super) fn make_date_ms(fields: DateFields, is_utc: bool) -> Option<f64> {
    let year = fields.year.checked_add(fields.month.div_euclid(12))?;
    let month = u32::try_from(fields.month.rem_euclid(12))
        .ok()?
        .saturating_add(1);
    let year = i32::try_from(year).ok()?;
    let mut date = NaiveDate::from_ymd_opt(year, month, 1)?;
    date = date.checked_add_signed(Duration::days(fields.day.checked_sub(1)?))?;
    let mut datetime = date.and_hms_opt(0, 0, 0)?;
    datetime = datetime.checked_add_signed(Duration::hours(fields.hour))?;
    datetime = datetime.checked_add_signed(Duration::minutes(fields.minute))?;
    datetime = datetime.checked_add_signed(Duration::seconds(fields.second))?;
    datetime = datetime.checked_add_signed(Duration::milliseconds(fields.millisecond))?;
    let milliseconds = if is_utc {
        Utc.from_utc_datetime(&datetime).timestamp_millis() as f64
    } else {
        let local = Local.from_local_datetime(&datetime);
        let local = local
            .single()
            .or_else(|| Local.from_local_datetime(&datetime).earliest())
            .or_else(|| Local.from_local_datetime(&datetime).latest())?;
        local.timestamp_millis() as f64
    };
    Some(milliseconds)
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

pub(super) fn time_clip(milliseconds: f64) -> f64 {
    if milliseconds.is_finite() && milliseconds.abs() <= 8.64e15 {
        milliseconds.trunc()
    } else {
        f64::NAN
    }
}

fn unix_time_millis() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(f64::NAN, |duration| duration.as_secs_f64() * 1_000.0)
}

pub(super) fn render_local_date(milliseconds: f64) -> String {
    wjsm_builtins::ms_to_datetime_local(milliseconds).map_or_else(
        || "Invalid Date".to_owned(),
        |date| date.format("%a %b %e %Y %H:%M:%S GMT%:z").to_string(),
    )
}

fn string_value(ctx: &mut NativeVmContext, state: &mut NativeAgentState, text: String) -> i64 {
    state
        .intern_text(text, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

pub(crate) fn parts(state: &mut NativeAgentState, encoded: i64) -> Option<(f64, u32)> {
    if !value::is_js_object(encoded) {
        return None;
    }
    let key = state.intern_property_string("__date_ms__".into())?;
    let milliseconds = state
        .gc
        .heap()
        .get_property(value::decode_handle(encoded), key)
        .ok()?? as i64;
    value::is_f64(milliseconds).then_some((
        value::decode_f64(milliseconds),
        value::decode_handle(encoded),
    ))
}

pub(crate) fn from_millis(state: &mut NativeAgentState, milliseconds: f64) -> Option<i64> {
    let object = state.allocate_object(1, false).ok()?;
    let stored = value::encode_f64(milliseconds);
    if modules::set_named_property(state, object, "__date_ms__", stored).is_err()
        || set_date_prototype(state, object).is_err()
    {
        return None;
    }
    Some(object)
}
