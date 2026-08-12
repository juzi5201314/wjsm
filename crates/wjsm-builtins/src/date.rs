use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

const DATE_METHODS: [(&str, &str); 43] = [
    ("getDate", "get_date"),
    ("getDay", "get_day"),
    ("getFullYear", "get_full_year"),
    ("getHours", "get_hours"),
    ("getMilliseconds", "get_milliseconds"),
    ("getMinutes", "get_minutes"),
    ("getMonth", "get_month"),
    ("getSeconds", "get_seconds"),
    ("getTime", "get_time"),
    ("getTimezoneOffset", "get_timezone_offset"),
    ("getUTCDate", "get_utc_date"),
    ("getUTCDay", "get_utc_day"),
    ("getUTCFullYear", "get_utc_full_year"),
    ("getUTCHours", "get_utc_hours"),
    ("getUTCMilliseconds", "get_utc_milliseconds"),
    ("getUTCMinutes", "get_utc_minutes"),
    ("getUTCMonth", "get_utc_month"),
    ("getUTCSeconds", "get_utc_seconds"),
    ("setDate", "set_date"),
    ("setFullYear", "set_full_year"),
    ("setHours", "set_hours"),
    ("setMilliseconds", "set_milliseconds"),
    ("setMinutes", "set_minutes"),
    ("setMonth", "set_month"),
    ("setSeconds", "set_seconds"),
    ("setTime", "set_time"),
    ("setUTCDate", "set_utc_date"),
    ("setUTCFullYear", "set_utc_full_year"),
    ("setUTCHours", "set_utc_hours"),
    ("setUTCMilliseconds", "set_utc_milliseconds"),
    ("setUTCMinutes", "set_utc_minutes"),
    ("setUTCMonth", "set_utc_month"),
    ("setUTCSeconds", "set_utc_seconds"),
    ("toString", "to_string"),
    ("toDateString", "to_date_string"),
    ("toTimeString", "to_time_string"),
    ("toLocaleString", "to_locale_string"),
    ("toLocaleDateString", "to_locale_date_string"),
    ("toLocaleTimeString", "to_locale_time_string"),
    ("toISOString", "to_iso_string"),
    ("toUTCString", "to_utc_string"),
    ("toJSON", "to_json"),
    ("valueOf", "value_of"),
];

pub fn constructor<E: ExecContext>(ctx: &mut E, args_base: i32, args_count: i32) -> Value {
    let args = shadow_args(ctx, args_base, args_count);
    if value::is_undefined(ctx.new_target()) {
        let rendered = render_local_date(ctx.date_now_ms());
        return ctx.store_string_owned(rendered);
    }

    let millis = constructor_millis(ctx, &args);
    let object = ctx.alloc_object(DATE_METHODS.len() as u32 + 1);
    ctx.set_date_prototype(object);
    ctx.define_data_property(object, "__date_ms__", value::encode_f64(millis));
    for &(name, kind) in &DATE_METHODS {
        let method = ctx.create_date_method(kind);
        ctx.define_data_property(object, name, method);
    }
    object
}

#[inline]
pub fn now<E: ExecContext>(ctx: &mut E) -> Value {
    value::encode_f64(ctx.date_now_ms())
}

pub fn parse<E: ExecContext>(ctx: &mut E, argument: Value) -> Value {
    if !value::is_string(argument) {
        return value::encode_f64(f64::NAN);
    }
    let source = ctx.read_string_utf8_lossy(argument);
    if source.is_empty() {
        return value::encode_f64(f64::NAN);
    }
    value::encode_f64(crate::date_parse::parse_date_string(&source).unwrap_or(f64::NAN))
}

pub fn utc<E: ExecContext>(ctx: &mut E, args_base: i32, args_count: i32) -> Value {
    let args = shadow_args(ctx, args_base, args_count);
    value::encode_f64(ctx.date_args_to_ms(&args, true))
}

fn constructor_millis<E: ExecContext>(ctx: &mut E, args: &[Value]) -> f64 {
    match args {
        [] => ctx.date_now_ms(),
        [argument] if value::is_undefined(*argument) => ctx.date_now_ms(),
        [argument] if value::is_f64(*argument) => {
            let number = value::decode_f64(*argument);
            if number.is_finite() { number } else { f64::NAN }
        }
        [argument] if value::is_string(*argument) => {
            let source = ctx.read_string_utf8_lossy(*argument);
            crate::date_parse::parse_date_string(&source).unwrap_or(f64::NAN)
        }
        [argument] if value::is_object(*argument) => ctx.date_read_ms(*argument),
        [_] => f64::NAN,
        _ => ctx.date_args_to_ms(args, false),
    }
}

fn shadow_args<E: ExecContext>(ctx: &mut E, args_base: i32, args_count: i32) -> Vec<Value> {
    (0..args_count.max(0))
        .map(|index| {
            ctx.read_call_arg(
                wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
                index as u32,
            )
        })
        .collect()
}

fn render_local_date(millis: f64) -> String {
    crate::date_parse::ms_to_datetime_local(millis)
        .map(|date| date.format("%a %b %e %Y %H:%M:%S GMT%:z").to_string())
        .unwrap_or_else(|| "Invalid Date".to_string())
}
