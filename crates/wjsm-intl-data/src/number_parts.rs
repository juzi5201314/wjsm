//! NumberFormat 成品字符串切分、补位与区间。

use icu::decimal::DecimalFormatter;
use icu::decimal::input::Decimal;
use std::str::FromStr;

use crate::format::FormatPart;
use crate::number::NumberFormatSpec;
use crate::number_symbols::{is_numbering_digit, substitute_digits};

pub(crate) struct SplitCtx<'a> {
    pub spec: &'a NumberFormatSpec,
    pub group: String,
    pub decimal: String,
}

pub(crate) fn separators(decimal: &DecimalFormatter, numbering_system: &str) -> (String, String) {
    let sample = decimal
        .format_to_string(&Decimal::from_str("1234.5").unwrap_or_else(|_| Decimal::from(1234)));
    let mut group = ",".to_owned();
    let mut dec = ".".to_owned();
    let chars: Vec<char> = sample.chars().collect();
    let seps: Vec<String> = chars
        .iter()
        .filter(|ch| !is_numbering_digit(**ch, numbering_system) && **ch != '-' && **ch != '+')
        .map(ToString::to_string)
        .collect();
    match seps.as_slice() {
        [first, second, ..] => {
            group = first.clone();
            dec = second.clone();
        }
        [only] => {
            let after = sample.rsplit_once(only.as_str()).map(|(_, rest)| rest);
            let digits_after = after
                .unwrap_or("")
                .chars()
                .filter(|ch| is_numbering_digit(*ch, numbering_system))
                .count();
            if digits_after == 1 {
                dec = only.clone();
            } else {
                group = only.clone();
            }
        }
        [] => {}
    }
    (group, dec)
}

pub(crate) fn split_formatted(text: &str, ctx: &SplitCtx<'_>) -> Vec<FormatPart> {
    let mut parts = Vec::new();
    if let Some(stripped) = text.strip_prefix('(')
        && let Some(inner) = stripped.strip_suffix(')')
    {
        parts.push(part("literal", "("));
        parts.extend(split_signed(inner, ctx));
        parts.push(part("literal", ")"));
        return parts
            .into_iter()
            .filter(|item| !item.value.is_empty())
            .collect();
    }
    parts.extend(split_signed(text, ctx));
    parts
        .into_iter()
        .filter(|item| !item.value.is_empty())
        .collect()
}

pub(crate) fn pad_integer_digits(text: &str, min_int: u32, numbering_system: &str) -> String {
    let zero = substitute_digits("0", numbering_system);
    let mut chars: Vec<char> = text.chars().collect();
    let start = chars
        .iter()
        .position(|ch| is_numbering_digit(*ch, numbering_system))
        .unwrap_or(0);
    let end = chars
        .iter()
        .skip(start)
        .position(|ch| !is_numbering_digit(*ch, numbering_system))
        .map(|offset| start + offset)
        .unwrap_or(chars.len());
    let have = end.saturating_sub(start);
    if have >= min_int as usize {
        return text.to_owned();
    }
    let extra = min_int as usize - have;
    let insert: Vec<char> = zero.chars().cycle().take(extra).collect();
    chars.splice(start..start, insert);
    chars.into_iter().collect()
}

pub(crate) fn trim_fraction_zeros(
    text: &str,
    decimal_sep: &str,
    min_frac: u32,
    numbering_system: &str,
    skip: bool,
) -> String {
    if skip || decimal_sep.is_empty() {
        return text.to_owned();
    }
    let Some(dot) = text.rfind(decimal_sep) else {
        return text.to_owned();
    };
    let head = &text[..dot];
    let mut frac = text[dot + decimal_sep.len()..].to_owned();
    let zero = substitute_digits("0", numbering_system);
    let zero_len = zero.chars().count();
    while frac.ends_with(&zero) {
        let digits = frac
            .chars()
            .filter(|ch| is_numbering_digit(*ch, numbering_system))
            .count();
        if digits <= min_frac as usize {
            break;
        }
        for _ in 0..zero_len {
            frac.pop();
        }
    }
    if frac.is_empty() {
        head.to_owned()
    } else {
        format!("{head}{decimal_sep}{frac}")
    }
}

pub(crate) fn apply_currency_code(formatted: &str, code: &str) -> String {
    const SYMBOLS: &[&str] = &[
        "US$", "CA$", "A$", "NZ$", "HK$", "MX$", "€", "£", "¥", "₩", "₹", "₽", "₺", "$",
    ];
    for symbol in SYMBOLS {
        if formatted.contains(symbol) {
            return formatted.replacen(symbol, code, 1);
        }
    }
    format!("{formatted} {code}")
}

pub(crate) fn build_range(
    spec: &NumberFormatSpec,
    start_text: &str,
    start_parts: Vec<FormatPart>,
    end_text: &str,
    end_parts: Vec<FormatPart>,
) -> (String, Vec<FormatPart>) {
    if start_text == end_text {
        let mut parts = vec![part_source("approximatelySign", "~", "shared")];
        parts.extend(with_source(start_parts, "shared"));
        return (format!("~{start_text}"), parts);
    }
    let start_affix = split_affix(start_text, spec);
    let end_affix = split_affix(end_text, spec);
    let share_suffix = !start_affix.suffix.is_empty() && start_affix.suffix == end_affix.suffix;
    let share_prefix = !start_affix.prefix.is_empty() && start_affix.prefix == end_affix.prefix;
    let collapse_prefix = share_prefix
        && !share_suffix
        && matches!(spec.sign_display.as_str(), "always" | "exceptZero");
    let sep = range_separator(spec, share_suffix, collapse_prefix);
    if share_suffix {
        let text = format!(
            "{}{}{sep}{}{}",
            start_affix.prefix, start_affix.number, end_affix.number, start_affix.suffix
        );
        let parts = range_parts_shared_suffix(&start_parts, &end_parts, &sep, &start_affix.suffix);
        (text, parts)
    } else if collapse_prefix {
        let text = format!(
            "{}{}{sep}{}{}",
            start_affix.prefix, start_affix.number, end_affix.number, start_affix.suffix
        );
        let parts =
            range_parts_collapsed_prefix(&start_parts, &end_parts, &sep, &start_affix.prefix);
        (text, parts)
    } else {
        let text = format!("{start_text}{sep}{end_text}");
        let mut parts = with_source(start_parts, "startRange");
        parts.push(part_source("literal", &sep, "shared"));
        parts.extend(with_source(end_parts, "endRange"));
        (text, parts)
    }
}

struct Affix<'a> {
    prefix: &'a str,
    number: &'a str,
    suffix: &'a str,
}

fn split_affix<'a>(text: &'a str, spec: &NumberFormatSpec) -> Affix<'a> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let first = chars
        .iter()
        .position(|(_, ch)| is_numbering_digit(*ch, &spec.numbering_system));
    let last = chars
        .iter()
        .rposition(|(_, ch)| is_numbering_digit(*ch, &spec.numbering_system));
    let Some(first) = first else {
        return Affix {
            prefix: text,
            number: "",
            suffix: "",
        };
    };
    let last = last.unwrap_or(first);
    let start = chars[first].0;
    let end = chars[last].0 + chars[last].1.len_utf8();
    Affix {
        prefix: &text[..start],
        number: &text[start..end],
        suffix: &text[end..],
    }
}

fn range_separator(spec: &NumberFormatSpec, share_suffix: bool, collapse_prefix: bool) -> String {
    if collapse_prefix {
        return "–".into();
    }
    if share_suffix {
        return " - ".into();
    }
    let lang = spec
        .locale
        .split(['-', '_'])
        .next()
        .unwrap_or(spec.locale.as_str());
    if spec.style == "currency" && lang == "en" {
        return " – ".into();
    }
    if lang == "en" {
        return "–".into();
    }
    " - ".into()
}

fn range_parts_shared_suffix(
    start: &[FormatPart],
    end: &[FormatPart],
    sep: &str,
    suffix: &str,
) -> Vec<FormatPart> {
    let start_core = strip_suffix_parts(start, suffix);
    let end_core = strip_prefix_matching(strip_suffix_parts(end, suffix), start);
    let mut parts = with_source(start_core, "startRange");
    parts.push(part_source("literal", sep, "shared"));
    parts.extend(with_source(end_core, "endRange"));
    parts.extend(with_source(suffix_parts(start, suffix), "shared"));
    parts
}

fn range_parts_collapsed_prefix(
    start: &[FormatPart],
    end: &[FormatPart],
    sep: &str,
    prefix: &str,
) -> Vec<FormatPart> {
    let (head, start_rest) = split_prefix_parts(start, prefix);
    let end_rest = skip_prefix_parts(end, prefix);
    let mut parts = with_source(head, "startRange");
    parts.extend(with_source(start_rest, "startRange"));
    parts.push(part_source("literal", sep, "shared"));
    parts.extend(with_source(end_rest, "endRange"));
    parts
}

fn suffix_parts(parts: &[FormatPart], suffix: &str) -> Vec<FormatPart> {
    let mut taken = String::new();
    let mut out = Vec::new();
    for part in parts.iter().rev() {
        let next = format!("{}{taken}", part.value);
        if suffix.ends_with(&next) {
            out.push(part.clone());
            taken = next;
            if taken == suffix {
                break;
            }
        } else {
            break;
        }
    }
    out.reverse();
    out
}

fn strip_suffix_parts(parts: &[FormatPart], suffix: &str) -> Vec<FormatPart> {
    let drop = suffix_parts(parts, suffix).len();
    parts[..parts.len().saturating_sub(drop)].to_vec()
}

fn strip_prefix_matching(parts: Vec<FormatPart>, start: &[FormatPart]) -> Vec<FormatPart> {
    let mut index = 0;
    while index < parts.len()
        && index < start.len()
        && parts[index].type_name == start[index].type_name
        && parts[index].value == start[index].value
        && matches!(
            parts[index].type_name.as_str(),
            "plusSign" | "minusSign" | "currency" | "literal"
        )
    {
        index += 1;
    }
    parts[index..].to_vec()
}

fn split_prefix_parts(parts: &[FormatPart], prefix: &str) -> (Vec<FormatPart>, Vec<FormatPart>) {
    let mut taken = String::new();
    let mut index = 0;
    while index < parts.len() {
        let next = format!("{taken}{}", parts[index].value);
        if prefix.starts_with(&next) {
            taken = next;
            index += 1;
            if taken == prefix {
                break;
            }
        } else {
            break;
        }
    }
    (parts[..index].to_vec(), parts[index..].to_vec())
}

fn skip_prefix_parts(parts: &[FormatPart], prefix: &str) -> Vec<FormatPart> {
    split_prefix_parts(parts, prefix).1
}

fn with_source(parts: Vec<FormatPart>, source: &str) -> Vec<FormatPart> {
    parts
        .into_iter()
        .map(|mut part| {
            part.source = Some(source.into());
            part
        })
        .collect()
}

fn split_signed(text: &str, ctx: &SplitCtx<'_>) -> Vec<FormatPart> {
    let mut parts = Vec::new();
    let mut rest = text;
    if let Some(stripped) = rest.strip_prefix('+') {
        parts.push(part("plusSign", "+"));
        rest = stripped;
    } else if let Some(stripped) = rest.strip_prefix('-') {
        parts.push(part("minusSign", "-"));
        rest = stripped;
    } else if let Some(stripped) = rest.strip_prefix('~') {
        parts.push(part("approximatelySign", "~"));
        rest = stripped;
    }
    let (prefix, core, suffix) = numeric_span(rest, ctx);
    parts.extend(classify_affix(prefix, ctx, true));
    parts.extend(split_number(core, ctx));
    parts.extend(classify_affix(suffix, ctx, false));
    parts
}

fn numeric_span<'a>(text: &'a str, ctx: &SplitCtx<'_>) -> (&'a str, &'a str, &'a str) {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let first = chars
        .iter()
        .position(|(_, ch)| is_numbering_digit(*ch, &ctx.spec.numbering_system));
    let last = chars
        .iter()
        .rposition(|(_, ch)| is_numbering_digit(*ch, &ctx.spec.numbering_system));
    let Some(first) = first else {
        return (text, "", "");
    };
    let last = last.unwrap_or(first);
    let start = chars[first].0;
    let end = chars[last].0 + chars[last].1.len_utf8();
    (&text[..start], &text[start..end], &text[end..])
}

fn classify_affix(text: &str, ctx: &SplitCtx<'_>, prefix: bool) -> Vec<FormatPart> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let trimmed_start = text.trim_start();
    let lead_len = text.len() - trimmed_start.len();
    if lead_len > 0 {
        parts.push(part("literal", &text[..lead_len]));
    }
    // 前缀可能是「单位 + 空白 + 符号」（시속 -987）；符号要单独剥成 plus/minus。
    let (body, sign) = peel_trailing_sign(trimmed_start);
    let trimmed = body.trim_end();
    let mid_ws = &body[trimmed.len()..];
    if !trimmed.is_empty() {
        parts.push(part(affix_type(trimmed, ctx, prefix), trimmed));
    }
    if !mid_ws.is_empty() {
        parts.push(part("literal", mid_ws));
    }
    if let Some(sign) = sign {
        parts.push(part(
            if sign == '+' { "plusSign" } else { "minusSign" },
            if sign == '+' { "+" } else { "-" },
        ));
    }
    parts
}

fn peel_trailing_sign(text: &str) -> (&str, Option<char>) {
    if let Some(body) = text.strip_suffix('-') {
        (body, Some('-'))
    } else if let Some(body) = text.strip_suffix('+') {
        (body, Some('+'))
    } else {
        (text, None)
    }
}

fn affix_type(token: &str, ctx: &SplitCtx<'_>, prefix: bool) -> &'static str {
    if ctx.spec.style == "currency" {
        return "currency";
    }
    if ctx.spec.style == "unit" {
        return "unit";
    }
    if ctx.spec.notation == "compact" && !prefix {
        return "compact";
    }
    if token == "%" {
        return if ctx.spec.style == "unit" {
            "unit"
        } else {
            "percentSign"
        };
    }
    "literal"
}

fn split_number(text: &str, ctx: &SplitCtx<'_>) -> Vec<FormatPart> {
    let mut parts = Vec::new();
    let mut digits = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if is_numbering_digit(ch, &ctx.spec.numbering_system) {
            digits.push(ch);
            continue;
        }
        if !digits.is_empty() {
            parts.push(part("integer", &digits));
            digits.clear();
        }
        let token = ch.to_string();
        if token == ctx.decimal {
            let mut fraction = String::new();
            while let Some(next) = chars.peek() {
                if is_numbering_digit(*next, &ctx.spec.numbering_system) {
                    fraction.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            parts.push(part("decimal", &token));
            if !fraction.is_empty() {
                parts.push(part("fraction", &fraction));
            }
        } else if token == ctx.group
            || chars
                .peek()
                .is_some_and(|next| is_numbering_digit(*next, &ctx.spec.numbering_system))
        {
            // 紧挨两组整数的分隔符即 grouping；不能只信 separators()，
            // compact 样本可能把 group 认成别的字符（de-DE 的 `.`）。
            parts.push(part("group", &token));
        } else {
            parts.push(part("literal", &token));
        }
    }
    if !digits.is_empty() {
        parts.push(part("integer", &digits));
    }
    parts
}

fn part(type_name: &str, value: &str) -> FormatPart {
    FormatPart {
        type_name: type_name.into(),
        value: value.into(),
        source: None,
        unit: None,
    }
}

fn part_source(type_name: &str, value: &str, source: &str) -> FormatPart {
    FormatPart {
        type_name: type_name.into(),
        value: value.into(),
        source: Some(source.into()),
        unit: None,
    }
}
