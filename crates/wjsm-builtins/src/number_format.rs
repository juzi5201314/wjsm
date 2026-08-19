//! Number / Math 纯格式化（ECMAScript ToString / toFixed / toExponential / toPrecision / 非 10 进制）。

fn normalize_negative_zero(x: f64) -> f64 {
    if x == 0.0 && x.is_sign_negative() {
        0.0
    } else {
        x
    }
}

/// 规范化科学计数法指数段（`e+N` / `e-N`）。
pub fn normalize_exponent(s: &str) -> String {
    if let Some(pos) = s.find('e') {
        let mantissa = &s[..pos];
        let exp_part = &s[pos + 1..];
        let exp_val: i32 = exp_part.parse().unwrap_or(0);
        format!(
            "{}e{}{}",
            mantissa,
            if exp_val >= 0 { "+" } else { "" },
            exp_val
        )
    } else if let Some(pos) = s.find('E') {
        let mantissa = &s[..pos];
        let exp_part = &s[pos + 1..];
        let exp_val: i32 = exp_part.parse().unwrap_or(0);
        format!(
            "{}e{}{}",
            mantissa,
            if exp_val >= 0 { "+" } else { "" },
            exp_val
        )
    } else {
        s.to_string()
    }
}

/// ECMAScript Number ToString 默认格式。
pub fn format_number_js(x: f64) -> String {
    if x == 0.0 {
        return "0".to_string();
    }
    let abs = x.abs();
    if abs >= 1e21 || (abs < 1e-6 && abs > 0.0) {
        let s = format!("{:e}", x);
        return normalize_exponent(&s);
    }
    format!("{}", x)
}

pub fn format_number_js_to_units(x: f64, out: &mut Vec<u16>) {
    if x == 0.0 {
        out.extend([b'0' as u16]);
        return;
    }
    if x.is_nan() {
        out.extend("NaN".encode_utf16());
        return;
    }
    if x.is_infinite() {
        if x > 0.0 {
            out.extend("Infinity".encode_utf16());
        } else {
            out.extend("-Infinity".encode_utf16());
        }
        return;
    }
    if write_integer_fast_to_units(x, out) {
        return;
    }
    let abs = x.abs();
    if abs >= 1e21 || (abs < 1e-6 && abs > 0.0) {
        let mut buf = ryu::Buffer::new();
        let s = buf.format(x);
        write_normalized_exp_to_units(s, out);
        return;
    }
    let mut buf = ryu::Buffer::new();
    let s = buf.format(x);
    out.extend(s.encode_utf16());
}

pub fn number_to_utf16_units_fast(x: f64) -> Vec<u16> {
    if x.is_nan() {
        return "NaN".encode_utf16().collect();
    }
    if x.is_infinite() {
        return if x > 0.0 {
            "Infinity".encode_utf16().collect()
        } else {
            "-Infinity".encode_utf16().collect()
        };
    }
    let mut out = Vec::new();
    format_number_js_to_units(x, &mut out);
    out
}

fn write_integer_fast_to_units(x: f64, out: &mut Vec<u16>) -> bool {
    if x.fract() != 0.0 {
        return false;
    }
    let abs = x.abs();
    if abs >= 1e21 || abs >= 9_007_199_254_740_992.0 {
        return false;
    }
    if abs < 1.0 && x != 0.0 {
        return false;
    }
    let trunc = x.trunc() as i64;
    if trunc == 0 {
        out.push(b'0' as u16);
        return true;
    }
    let negative = trunc < 0;
    let mut value = if negative {
        (-(trunc as i128)) as u64
    } else {
        trunc as u64
    };
    let mut digits = [0u8; 20];
    let mut len = 0;
    while value > 0 {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    if negative {
        out.push(b'-' as u16);
    }
    for idx in (0..len).rev() {
        out.push(digits[idx] as u16);
    }
    true
}

fn write_normalized_exp_to_units(s: &str, out: &mut Vec<u16>) {
    if let Some(pos) = s.find(['e', 'E']) {
        let mantissa = &s[..pos];
        let exp_part = &s[pos + 1..];
        let exp_val: i32 = exp_part.parse().unwrap_or(0);
        out.extend(mantissa.encode_utf16());
        out.push(b'e' as u16);
        if exp_val >= 0 {
            out.push(b'+' as u16);
        }
        out.extend(exp_val.to_string().encode_utf16());
    } else {
        out.extend(s.encode_utf16());
    }
}

/// ECMA-262 §21.1.3.3 Number.prototype.toFixed。
pub fn format_number_to_fixed_js(x: f64, digits: i32) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    // ECMA-262 §21.1.3.3 step 8: x ≥ 10^21 → ToString(x)
    if x.abs() >= 1e21 {
        return format_number_js(x);
    }
    let x = normalize_negative_zero(x);
    format!("{:.1$}", x, digits as usize)
}

/// ECMA-262 §21.1.3.2 Number.prototype.toExponential。
pub fn format_number_to_exponential_js(x: f64, digits: Option<i32>) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let x = normalize_negative_zero(x);
    if x == 0.0 {
        if let Some(digits) = digits
            && digits > 0
        {
            return format!("0.{}e+0", "0".repeat(digits as usize));
        }
        return "0e+0".to_string();
    }
    let s = if let Some(digits) = digits {
        format!("{:.1$e}", x, digits as usize)
    } else {
        format!("{:e}", x)
    };
    normalize_exponent(&s)
}

/// ECMA-262 §21.1.3.5 Number.prototype.toPrecision。
pub fn format_number_to_precision_js(x: f64, precision: Option<i32>) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let x = normalize_negative_zero(x);
    let Some(precision) = precision else {
        return format_number_js(x);
    };
    if x == 0.0 {
        if precision == 1 {
            return "0".to_string();
        }
        return format!("0.{}", "0".repeat((precision - 1) as usize));
    }
    let exponent = x.abs().log10().floor() as i32;
    if exponent >= precision || exponent < -6 {
        let s = format!("{:.1$e}", x, (precision - 1) as usize);
        return normalize_exponent(&s);
    }
    let fraction_digits = (precision - exponent - 1).max(0) as usize;
    format!("{:.1$}", x, fraction_digits)
}

fn format_u64_radix(mut value: u64, radix: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut result = Vec::new();
    let r = radix as u64;
    while value > 0 {
        result.push(digits[(value % r) as usize]);
        value /= r;
    }
    result.reverse();
    String::from_utf8(result).unwrap_or_else(|_| "0".to_string())
}

fn format_f64_uint_radix(mut value: f64, radix: u32) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let r = radix as f64;
    let mut result = Vec::new();
    while value >= 1.0 {
        let rem = (value % r).trunc();
        let digit = rem as usize;
        if digit >= radix as usize {
            break;
        }
        result.push(digits[digit]);
        value = (value / r).trunc();
        if value.is_nan() || value.is_infinite() {
            break;
        }
    }
    if result.is_empty() {
        return "0".to_string();
    }
    result.reverse();
    String::from_utf8(result).unwrap_or_else(|_| "0".to_string())
}

/// ECMA-262 §21.1.3.6 Number.prototype.toString(radix)（非 10 进制）。
pub fn number_proto_to_string_radix(x: f64, radix: i32) -> String {
    if x == 0.0 && !x.is_sign_negative() {
        return "0".to_string();
    }
    let radix_u = radix as u32;
    let negative = x.is_sign_negative();
    let abs_x = x.abs();
    let int_whole = abs_x.trunc();
    let mut int_str = if int_whole == 0.0 {
        "0".to_string()
    } else if int_whole <= u64::MAX as f64 {
        format_u64_radix(int_whole as u64, radix_u)
    } else {
        format_f64_uint_radix(int_whole, radix_u)
    };
    let mut frac = abs_x - int_whole;
    if frac > 0.0 {
        int_str.push('.');
        let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
        const MAX_FRAC_DIGITS: usize = 52;
        for _ in 0..MAX_FRAC_DIGITS {
            if frac == 0.0 {
                break;
            }
            frac *= radix_u as f64;
            let digit = frac.trunc() as usize;
            if digit >= radix as usize {
                break;
            }
            int_str.push(digits[digit] as char);
            frac -= digit as f64;
        }
    }
    if negative {
        format!("-{int_str}")
    } else {
        int_str
    }
}
