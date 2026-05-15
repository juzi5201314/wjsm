use wjsm_ir::value;

pub(crate) fn format_number_js(x: f64) -> String {
    if x == 0.0 {
        return "0".to_string();
    }
    let abs = x.abs();
    if abs >= 1e21 || (abs < 1e-6 && abs > 0.0) {
        let s = format!("{:e}", x);
        return normalize_exponent(&s);
    }
    let s = format!("{}", x);
    s
}

pub(crate) fn format_radix(mut value: i64, radix: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }
    let negative = value < 0;
    if negative {
        value = -value;
    }
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut result = Vec::new();
    while value > 0 {
        result.push(digits[value as usize % radix as usize]);
        value /= radix as i64;
    }
    if negative {
        result.push(b'-');
    }
    result.reverse();
    String::from_utf8(result).unwrap_or_else(|_| "0".to_string())
}

pub(crate) fn normalize_exponent(s: &str) -> String {
    if let Some(pos) = s.find('e') {
        let mantissa = &s[..pos];
        let exp_part = &s[pos + 1..];
        let exp_val: i32 = exp_part.parse().unwrap_or(0);
        format!("{}e{}{}", mantissa, if exp_val >= 0 { "+" } else { "" }, exp_val)
    } else if let Some(pos) = s.find('E') {
        let mantissa = &s[..pos];
        let exp_part = &s[pos + 1..];
        let exp_val: i32 = exp_part.parse().unwrap_or(0);
        format!("{}e{}{}", mantissa, if exp_val >= 0 { "+" } else { "" }, exp_val)
    } else {
        s.to_string()
    }
}

