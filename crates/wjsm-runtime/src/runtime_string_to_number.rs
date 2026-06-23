//! ECMAScript §7.1.4 String 的 ToNumber（StringToNumber）纯字符串实现。

/// JS 空白：StrWhiteSpaceChar + StrLineTerminator
fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000B}'
            | '\u{000C}'
            | ' '
            | '\u{00A0}'
            | '\u{FEFF}'
            | '\n'
            | '\r'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// 去掉首尾 ECMAScript 空白与行终止符。
pub(crate) fn trim_js_whitespace(s: &str) -> &str {
    let start = s
        .char_indices()
        .find(|(_, c)| !is_js_whitespace(*c))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let end = s
        .char_indices()
        .rfind(|(_, c)| !is_js_whitespace(*c))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(start);
    &s[start..end]
}

/// 将已 trim 的字符串按 StringNumericLiteral 风格转为 f64。
pub(crate) fn string_to_f64(trimmed: &str) -> f64 {
    if trimmed.is_empty() {
        return 0.0;
    }
    if trimmed == "Infinity" {
        return f64::INFINITY;
    }
    if trimmed == "-Infinity" {
        return f64::NEG_INFINITY;
    }

    let (radix, digits): (u32, &str) = if let Some(rest) = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")) {
        (16, rest)
    } else if let Some(rest) = trimmed.strip_prefix("0o").or_else(|| trimmed.strip_prefix("0O")) {
        (8, rest)
    } else if let Some(rest) = trimmed.strip_prefix("0b").or_else(|| trimmed.strip_prefix("0B")) {
        (2, rest)
    } else {
        let s = trimmed.strip_prefix('+').unwrap_or(trimmed);
        return match s.parse::<f64>() {
            Ok(n) => n,
            Err(_) => f64::NAN,
        };
    };

    if digits.is_empty() {
        return f64::NAN;
    }
    if !digits.chars().all(|c| char_valid_for_radix(c, radix)) {
        return f64::NAN;
    }
    match u64::from_str_radix(digits, radix) {
        Ok(v) => v as f64,
        Err(_) => f64::NAN,
    }
}

fn char_valid_for_radix(c: char, radix: u32) -> bool {
    match radix {
        16 => c.is_ascii_digit() || matches!(c, 'a'..='f' | 'A'..='F'),
        8 => matches!(c, '0'..='7'),
        2 => matches!(c, '0' | '1'),
        _ => c.is_ascii_digit(),
    }
}

/// 对任意 JS 字符串内容执行 ToNumber。
pub(crate) fn js_string_content_to_f64(s: &str) -> f64 {
    string_to_f64(trim_js_whitespace(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_to_f64_cases() {
        assert_eq!(js_string_content_to_f64("123"), 123.0);
        assert_eq!(js_string_content_to_f64("0x10"), 16.0);
        assert_eq!(js_string_content_to_f64("0o17"), 15.0);
        assert_eq!(js_string_content_to_f64("0b101"), 5.0);
        assert_eq!(js_string_content_to_f64(""), 0.0);
        assert_eq!(js_string_content_to_f64("  \t\n  "), 0.0);
        assert!(js_string_content_to_f64("abc").is_nan());
        assert_eq!(js_string_content_to_f64("3.14"), 3.14);
        assert_eq!(js_string_content_to_f64("1e3"), 1000.0);
        assert_eq!(js_string_content_to_f64("  42  "), 42.0);
        assert_eq!(js_string_content_to_f64("Infinity"), f64::INFINITY);
        assert_eq!(js_string_content_to_f64("-Infinity"), f64::NEG_INFINITY);
        assert!(js_string_content_to_f64("123abc").is_nan());
    }
}