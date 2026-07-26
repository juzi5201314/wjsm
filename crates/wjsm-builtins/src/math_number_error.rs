//! Math / Number / Boolean / Error 宿主 builtin。

use num_traits::ToPrimitive;
use rand::Rng;
use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

use crate::{
    format_number_js, format_number_to_exponential_js, format_number_to_fixed_js,
    format_number_to_precision_js, js_string_content_to_f64, number_proto_to_string_radix,
};

fn math_decode_f64_arg<E: ExecContext>(ctx: &mut E, arg: Value) -> Result<f64, Value> {
    let num = ctx.value_to_number(arg);
    if value::is_exception(num) {
        Err(num)
    } else {
        Ok(value::decode_f64(num))
    }
}

fn parse_int_digit_value(c: char, radix: u32) -> Option<u32> {
    let digit = if c.is_ascii_digit() {
        c as u32 - b'0' as u32
    } else if c.is_ascii_alphabetic() {
        c.to_ascii_lowercase() as u32 - b'a' as u32 + 10
    } else {
        return None;
    };
    if digit < radix { Some(digit) } else { None }
}

fn parse_int_take_valid_prefix(s: &str, radix: u32) -> String {
    s.chars()
        .take_while(|c| parse_int_digit_value(*c, radix).is_some())
        .collect()
}

fn parse_int_radix_and_body(trimmed: &str, radix: i32) -> (i32, &str) {
    if radix == 0 {
        if trimmed.starts_with("0b") || trimmed.starts_with("0B") {
            (2, &trimmed[2..])
        } else if trimmed.starts_with("0o") || trimmed.starts_with("0O") {
            (8, &trimmed[2..])
        } else if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
            (16, &trimmed[2..])
        } else {
            (10, trimmed)
        }
    } else {
        let body = if radix == 16 && (trimmed.starts_with("0x") || trimmed.starts_with("0X")) {
            &trimmed[2..]
        } else {
            trimmed
        };
        (radix, body)
    }
}

pub fn math_abs<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.abs())
}

pub fn math_acos<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.acos())
}

pub fn math_acosh<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.acosh())
}

pub fn math_asin<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.asin())
}

pub fn math_asinh<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.asinh())
}

pub fn math_atan<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.atan())
}

pub fn math_atanh<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.atanh())
}

pub fn math_cbrt<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.cbrt())
}

pub fn math_ceil<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.ceil())
}

pub fn math_cos<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.cos())
}

pub fn math_cosh<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.cosh())
}

pub fn math_exp<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.exp())
}

pub fn math_expm1<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.exp_m1())
}

pub fn math_floor<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.floor())
}

pub fn math_fround<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64((x as f32) as f64)
}

pub fn math_log<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.ln())
}

pub fn math_log1p<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.ln_1p())
}

pub fn math_log10<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.log10())
}

pub fn math_log2<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.log2())
}

pub fn math_sin<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.sin())
}

pub fn math_sinh<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.sinh())
}

pub fn math_sqrt<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.sqrt())
}

pub fn math_tan<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.tan())
}

pub fn math_tanh<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.tanh())
}

pub fn math_trunc<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(x.trunc())
}

pub fn math_atan2<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    let y = match math_decode_f64_arg(ctx, a) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let x = match math_decode_f64_arg(ctx, b) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(y.atan2(x))
}

pub fn math_clz32<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let n = x as i32 as u32;
    value::encode_f64(if n == 0 { 32.0 } else { n.leading_zeros() as f64 })
}

pub fn math_hypot<E: ExecContext>(ctx: &mut E, args_base: i32, args_count: i32) -> Value {
    if args_count == 0 {
        return value::encode_f64(0.0);
    }
    let mut sum = 0.0_f64;
    for i in 0..args_count as u32 {
        let val = ctx.read_shadow_arg(args_base, i);
        let x = match math_decode_f64_arg(ctx, val) {
            Ok(v) => v,
            Err(e) => return e,
        };
        if x.is_infinite() {
            return value::encode_f64(f64::INFINITY);
        }
        sum += x * x;
    }
    value::encode_f64(sum.sqrt())
}

pub fn math_imul<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    let ai = match math_decode_f64_arg(ctx, a) {
        Ok(v) => v,
        Err(e) => return e,
    } as i32;
    let bi = match math_decode_f64_arg(ctx, b) {
        Ok(v) => v,
        Err(e) => return e,
    } as i32;
    let result = (ai as i64) * (bi as i64);
    value::encode_f64((result as i32) as f64)
}

pub fn math_max<E: ExecContext>(ctx: &mut E, args_base: i32, args_count: i32) -> Value {
    if args_count == 0 {
        return value::encode_f64(f64::NEG_INFINITY);
    }
    let mut result = f64::NEG_INFINITY;
    for i in 0..args_count as u32 {
        let val = ctx.read_shadow_arg(args_base, i);
        let x = match math_decode_f64_arg(ctx, val) {
            Ok(v) => v,
            Err(e) => return e,
        };
        if x.is_nan() {
            return value::encode_f64(f64::NAN);
        }
        if x > result || (x == 0.0 && result == 0.0 && x.is_sign_negative()) {
            result = x;
        }
    }
    value::encode_f64(result)
}

pub fn math_min<E: ExecContext>(ctx: &mut E, args_base: i32, args_count: i32) -> Value {
    if args_count == 0 {
        return value::encode_f64(f64::INFINITY);
    }
    let mut result = f64::INFINITY;
    for i in 0..args_count as u32 {
        let val = ctx.read_shadow_arg(args_base, i);
        let x = match math_decode_f64_arg(ctx, val) {
            Ok(v) => v,
            Err(e) => return e,
        };
        if x.is_nan() {
            return value::encode_f64(f64::NAN);
        }
        if x < result || (x == 0.0 && result == 0.0 && x.is_sign_positive()) {
            result = x;
        }
    }
    value::encode_f64(result)
}

pub fn math_pow<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    let base = match math_decode_f64_arg(ctx, a) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let exp = match math_decode_f64_arg(ctx, b) {
        Ok(v) => v,
        Err(e) => return e,
    };
    value::encode_f64(base.powf(exp))
}

pub fn math_random<E: ExecContext>(_ctx: &mut E) -> Value {
    let mut rng = rand::thread_rng();
    value::encode_f64(rng.gen_range(0.0_f64..1.0))
}

pub fn math_round<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if x.is_nan() || x.is_infinite() {
        return value::encode_f64(x);
    }
    if x == 0.0 {
        return value::encode_f64(x);
    }
    let fl = x.floor();
    if fl + 0.5 <= x {
        value::encode_f64(fl + 1.0)
    } else {
        value::encode_f64(fl)
    }
}

pub fn math_sign<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let x = match math_decode_f64_arg(ctx, arg) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if x.is_nan() {
        return value::encode_f64(f64::NAN);
    }
    if x == 0.0 {
        return value::encode_f64(if x.is_sign_positive() { 0.0 } else { -0.0 });
    }
    value::encode_f64(if x > 0.0 { 1.0 } else { -1.0 })
}


pub fn number_constructor<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    if value::is_f64(arg) {
        return arg;
    }
    if value::is_undefined(arg) {
        return value::encode_f64(f64::NAN);
    }
    if value::is_null(arg) {
        return value::encode_f64(0.0);
    }
    if value::is_bool(arg) {
        return value::encode_f64(if value::decode_bool(arg) { 1.0 } else { 0.0 });
    }
    if value::is_string(arg) {
        let s = ctx.read_string_bytes(arg).unwrap_or_default();
        let s_str = String::from_utf8_lossy(&s).to_string();
        return value::encode_f64(js_string_content_to_f64(&s_str));
    }
    if value::is_bigint(arg) {
        return value::encode_f64(
            ctx.read_bigint(arg)
                .and_then(|bi| bi.to_f64())
                .unwrap_or(f64::NAN),
        );
    }
    if value::is_symbol(arg) {
        return ctx.make_type_error("Cannot convert a Symbol value to a number");
    }
    if value::is_object(arg) || value::is_callable(arg) {
        let prim = ctx.to_primitive(arg, true);
        if value::is_exception(prim) {
            return prim;
        }
        if value::is_bigint(prim) {
            return value::encode_f64(
                ctx.read_bigint(prim)
                    .and_then(|bi| bi.to_f64())
                    .unwrap_or(f64::NAN),
            );
        }
        return ctx.to_number(prim);
    }
    value::encode_f64(f64::NAN)
}

pub fn number_is_nan<E: ExecContext>(_ctx: &mut E, arg: Value) -> Value {
    if value::is_f64(arg) {
        value::encode_bool(value::decode_f64(arg).is_nan())
    } else if value::is_undefined(arg)
        || value::is_null(arg)
        || value::is_bool(arg)
        || value::is_string(arg)
        || value::is_object(arg)
        || value::is_function(arg)
        || value::is_closure(arg)
        || value::is_bound(arg)
        || value::is_bigint(arg)
        || value::is_symbol(arg)
        || value::is_regexp(arg)
        || value::is_array(arg)
        || value::is_iterator(arg)
        || value::is_enumerator(arg)
        || value::is_proxy(arg)
    {
        value::encode_bool(false)
    } else {
        value::encode_bool(true)
    }
}

pub fn number_is_finite<E: ExecContext>(_ctx: &mut E, arg: Value) -> Value {
    if value::is_f64(arg) {
        value::encode_bool(value::decode_f64(arg).is_finite())
    } else {
        value::encode_bool(false)
    }
}

pub fn number_is_integer<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    if value::is_f64(arg) {
        let x = match math_decode_f64_arg(ctx, arg) {
            Ok(v) => v,
            Err(e) => return e,
        };
        value::encode_bool(x.is_finite() && x == x.trunc())
    } else {
        value::encode_bool(false)
    }
}

pub fn number_is_safe_integer<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    if value::is_f64(arg) {
        let x = match math_decode_f64_arg(ctx, arg) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let is_int = x.is_finite() && x == x.trunc();
        value::encode_bool(is_int && x.abs() <= 9007199254740991.0)
    } else {
        value::encode_bool(false)
    }
}

pub fn number_parse_int<E: ExecContext>(ctx: &mut E, arg: Value, radix_val: Value) -> Value {
    let input_str = if value::is_string(arg) {
        let s = ctx.read_string_bytes(arg).unwrap_or_default();
        String::from_utf8_lossy(&s).to_string()
    } else if value::is_f64(arg) {
        let x = match math_decode_f64_arg(ctx, arg) {
            Ok(v) => v,
            Err(e) => return e,
        };
        if x.is_nan() || x.is_infinite() {
            return value::encode_f64(f64::NAN);
        }
        format_number_js(x)
    } else if value::is_bool(arg) {
        if value::decode_bool(arg) { "1" } else { "0" }.to_string()
    } else {
        return value::encode_f64(f64::NAN);
    };
    let trimmed = input_str.trim();
    if trimmed.is_empty() {
        return value::encode_f64(f64::NAN);
    }
    let radix = if value::is_undefined(radix_val) {
        0
    } else if value::is_f64(radix_val) {
        let r = value::decode_f64(radix_val);
        if r.is_nan() || r.is_infinite() {
            return value::encode_f64(f64::NAN);
        }
        r as i32
    } else {
        0
    };
    if radix != 0 && !(2..=36).contains(&radix) {
        return value::encode_f64(f64::NAN);
    }
    let mut core = trimmed;
    let mut sign = 1.0_f64;
    if let Some(rest) = core.strip_prefix('+') {
        core = rest;
    } else if let Some(rest) = core.strip_prefix('-') {
        core = rest;
        sign = -1.0;
    }
    let (actual_radix, parse_str) = parse_int_radix_and_body(core, radix);
    if parse_str.is_empty() {
        return value::encode_f64(f64::NAN);
    }
    let valid_chars = parse_int_take_valid_prefix(parse_str, actual_radix as u32);
    if valid_chars.is_empty() {
        return value::encode_f64(f64::NAN);
    }
    match i64::from_str_radix(&valid_chars, actual_radix as u32) {
        Ok(v) => value::encode_f64(sign * v as f64),
        Err(_) => value::encode_f64(f64::NAN),
    }
}

pub fn number_parse_float<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    if !value::is_string(arg) {
        if value::is_f64(arg) {
            return arg;
        }
        return value::encode_f64(f64::NAN);
    }
    let s = ctx.read_string_bytes(arg).unwrap_or_default();
    let s_str = String::from_utf8_lossy(&s).to_string();
    let trimmed = s_str.trim();
    if trimmed.is_empty() {
        return value::encode_f64(f64::NAN);
    }
    let bytes = trimmed.as_bytes();
    let mut sign: f64 = 1.0;
    let mut pos = 0usize;
    if pos < bytes.len() && (bytes[pos] == b'+' || bytes[pos] == b'-') {
        if bytes[pos] == b'-' {
            sign = -1.0;
        }
        pos += 1;
    }
    const INFINITY_PREFIX: &[u8] = b"Infinity";
    if bytes.len() >= pos + INFINITY_PREFIX.len()
        && bytes[pos..pos + INFINITY_PREFIX.len()] == *INFINITY_PREFIX
    {
        return value::encode_f64(sign * f64::INFINITY);
    }
    let mut end = 0;
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    let mut has_digit = false;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
        has_digit = true;
    }
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
            has_digit = true;
        }
    }
    if !has_digit {
        return value::encode_f64(f64::NAN);
    }
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        end += 1;
        if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
            end += 1;
        }
        let exp_start = end;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == exp_start {
            end -= if end > 0 && (bytes[end - 1] == b'+' || bytes[end - 1] == b'-') {
                1
            } else {
                0
            };
            if end > 0 && (bytes[end - 1] == b'e' || bytes[end - 1] == b'E') {
                end -= 1;
            }
        }
    }
    if end == 0 {
        return value::encode_f64(f64::NAN);
    }
    let float_str = &trimmed[..end];
    match float_str.parse::<f64>() {
        Ok(v) => value::encode_f64(v),
        Err(_) => value::encode_f64(f64::NAN),
    }
}

pub fn number_proto_to_string<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    radix_val: Value,
) -> Value {
    if !value::is_f64(this_val) {
        return ctx.store_string("NaN");
    }
    let x = value::decode_f64(this_val);
    let radix = if value::is_undefined(radix_val) || value::is_null(radix_val) {
        10
    } else if value::is_f64(radix_val) {
        let r = value::decode_f64(radix_val) as i32;
        if !(2..=36).contains(&r) {
            return ctx.make_range_error("toString() radix argument must be between 2 and 36");
        }
        r
    } else {
        10
    };
    if x.is_nan() {
        return ctx.store_string("NaN");
    }
    if x.is_infinite() {
        return ctx.store_string(if x > 0.0 { "Infinity" } else { "-Infinity" });
    }
    if radix == 10 {
        return ctx.store_string_owned(format_number_js(x));
    }
    ctx.store_string_owned(number_proto_to_string_radix(x, radix))
}

pub fn number_proto_value_of<E: ExecContext>(_ctx: &mut E, this_val: Value) -> Value {
    if value::is_f64(this_val) {
        this_val
    } else {
        value::encode_f64(0.0)
    }
}

pub fn number_proto_to_fixed<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    digits_val: Value,
) -> Value {
    if !value::is_f64(this_val) {
        return ctx.store_string("NaN");
    }
    let x = value::decode_f64(this_val);
    let digits = if value::is_undefined(digits_val) || value::is_null(digits_val) {
        0
    } else if value::is_f64(digits_val) {
        value::decode_f64(digits_val) as i32
    } else {
        0
    };
    if !(0..=100).contains(&digits) {
        return ctx.make_range_error("toFixed() digits argument must be between 0 and 100");
    }
    ctx.store_string_owned(format_number_to_fixed_js(x, digits))
}

pub fn number_proto_to_exponential<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    digits_val: Value,
) -> Value {
    if !value::is_f64(this_val) {
        return ctx.store_string("NaN");
    }
    let x = value::decode_f64(this_val);
    let digits = if value::is_undefined(digits_val) || value::is_null(digits_val) {
        None
    } else if value::is_f64(digits_val) {
        Some(value::decode_f64(digits_val) as i32)
    } else {
        None
    };
    if let Some(f) = digits
        && !(0..=100).contains(&f)
    {
        return ctx.make_range_error("toExponential() argument must be between 0 and 100");
    }
    ctx.store_string_owned(format_number_to_exponential_js(x, digits))
}

pub fn number_proto_to_precision<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    digits_val: Value,
) -> Value {
    if !value::is_f64(this_val) {
        return ctx.store_string("NaN");
    }
    let x = value::decode_f64(this_val);
    let precision = if value::is_undefined(digits_val) {
        None
    } else if value::is_f64(digits_val) {
        Some(value::decode_f64(digits_val) as i32)
    } else {
        Some(-1)
    };
    if let Some(precision) = precision
        && !(1..=100).contains(&precision)
    {
        return ctx.make_range_error("toPrecision() argument must be between 1 and 100");
    }
    ctx.store_string_owned(format_number_to_precision_js(x, precision))
}

pub fn boolean_constructor<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    value::encode_bool(ctx.to_boolean(arg))
}

pub fn boolean_proto_to_string<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    if value::is_bool(this_val) {
        ctx.store_string(if value::decode_bool(this_val) {
            "true"
        } else {
            "false"
        })
    } else {
        ctx.store_string("false")
    }
}

pub fn boolean_proto_value_of<E: ExecContext>(_ctx: &mut E, this_val: Value) -> Value {
    if value::is_bool(this_val) {
        this_val
    } else {
        value::encode_bool(false)
    }
}

pub fn error_constructor<E: ExecContext>(
    ctx: &mut E,
    arg: Value,
    options: Value,
) -> Value {
    ctx.create_error_object("Error", arg, options)
}
pub fn type_error_constructor<E: ExecContext>(ctx: &mut E, arg: Value, options: Value) -> Value {
    ctx.create_error_object("TypeError", arg, options)
}
pub fn range_error_constructor<E: ExecContext>(ctx: &mut E, arg: Value, options: Value) -> Value {
    ctx.create_error_object("RangeError", arg, options)
}
pub fn syntax_error_constructor<E: ExecContext>(ctx: &mut E, arg: Value, options: Value) -> Value {
    ctx.create_error_object("SyntaxError", arg, options)
}
pub fn reference_error_constructor<E: ExecContext>(
    ctx: &mut E,
    arg: Value,
    options: Value,
) -> Value {
    ctx.create_error_object("ReferenceError", arg, options)
}
pub fn uri_error_constructor<E: ExecContext>(ctx: &mut E, arg: Value, options: Value) -> Value {
    ctx.create_error_object("URIError", arg, options)
}
pub fn eval_error_constructor<E: ExecContext>(ctx: &mut E, arg: Value, options: Value) -> Value {
    ctx.create_error_object("EvalError", arg, options)
}

pub fn error_proto_to_string<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    ctx.error_proto_to_string(this_val)
}

/// BigInt 原始值的原型方法名 → NativeCallable。
pub fn primitive_bigint_get_method<E: ExecContext>(
    ctx: &mut E,
    boxed: Value,
    name_id: u32,
) -> Value {
    if !value::is_bigint(boxed) {
        return value::encode_undefined();
    }
    let method = match ctx.read_memory_string_bytes(name_id).as_slice() {
        b"toString" => 0,
        b"valueOf" => 1,
        _ => return value::encode_undefined(),
    };
    ctx.create_bigint_primitive_method(method)
}

/// raw f64 数字的原型方法名 → NativeCallable。
pub fn primitive_number_get_method<E: ExecContext>(
    ctx: &mut E,
    boxed: Value,
    name_id: u32,
) -> Value {
    if (boxed as u64 & value::BOX_BASE) == value::BOX_BASE {
        return value::encode_undefined();
    }
    let method = match ctx.read_memory_string_bytes(name_id).as_slice() {
        b"toString" => 0,
        b"valueOf" => 1,
        b"toFixed" => 2,
        b"toExponential" => 3,
        b"toPrecision" => 4,
        _ => return value::encode_undefined(),
    };
    ctx.create_number_primitive_method(method)
}

pub fn primitive_symbol_get_property<E: ExecContext>(
    ctx: &mut E,
    boxed: Value,
    name_id: u32,
) -> Value {
    ctx.primitive_symbol_get_property(boxed, name_id)
}

pub fn primitive_regexp_get_property<E: ExecContext>(
    ctx: &mut E,
    boxed: Value,
    name_id: u32,
) -> Value {
    ctx.primitive_regexp_get_property(boxed, name_id)
}

pub fn primitive_regexp_set_property<E: ExecContext>(
    ctx: &mut E,
    boxed: Value,
    name_id: u32,
    val: Value,
) {
    ctx.primitive_regexp_set_property(boxed, name_id, val)
}
