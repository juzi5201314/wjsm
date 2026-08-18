//! ECMA-402 `PartitionDurationFormatPattern`。
//!
//! 用 NumberFormat + ListFormat 拼出与 test262 `testIntl.js` 一致的 parts。

use crate::format::{FormatPart, OwnedListFormatter};
use crate::number::{NumberFormatSpec, OwnedNumberFormatter};

const UNITS: [&str; 10] = [
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

#[derive(Clone, Debug)]
pub struct DurationUnitSpec {
    pub style: String,
    pub display: String,
}

#[derive(Clone, Debug)]
pub struct DurationFormatSpec {
    pub locale: String,
    pub numbering_system: String,
    pub style: String,
    pub units: Vec<DurationUnitSpec>,
    pub fractional_digits: Option<u32>,
}

impl DurationFormatSpec {
    pub fn format(&self, values: &[f64; 10]) -> Result<String, String> {
        Ok(self
            .format_parts(values)?
            .into_iter()
            .map(|part| part.value)
            .collect())
    }

    pub fn format_parts(&self, values: &[f64; 10]) -> Result<Vec<FormatPart>, String> {
        let mut groups: Vec<Vec<FormatPart>> = Vec::new();
        let mut need_separator = false;
        let mut display_negative = true;
        for (index, name) in UNITS.iter().enumerate() {
            let unit = self.unit(index);
            let mut value = values[index];
            let mut fractional = None;
            let mut done = false;
            if matches!(*name, "seconds" | "milliseconds" | "microseconds")
                && matches!(
                    self.unit(index + 1).style.as_str(),
                    "numeric" | "fractional"
                )
            {
                let exponent = match *name {
                    "seconds" => 9,
                    "milliseconds" => 6,
                    _ => 3,
                };
                fractional = Some(duration_to_fractional(values, exponent));
                done = true;
            }
            let seconds_shown = self.unit(6).display == "always"
                || values[6] != 0.0
                || values[7] != 0.0
                || values[8] != 0.0
                || values[9] != 0.0;
            let display_required = *name == "minutes" && need_separator && seconds_shown;
            let numeric = fractional
                .as_deref()
                .map(|text| !is_zero_decimal(text))
                .unwrap_or(value != 0.0);
            if numeric || unit.display != "auto" || display_required {
                let mut sign_display = "never";
                if display_negative {
                    display_negative = false;
                    sign_display = "auto";
                    if value == 0.0 && fractional.is_none() {
                        // DurationSign：只有真正的负字段才把 0 收成 -0；输入 -0 本身不算负。
                        value = if values.iter().any(|item| *item < 0.0) {
                            -0.0
                        } else {
                            0.0
                        };
                    }
                }
                let raw = if let Some(text) = fractional {
                    text
                } else {
                    decimal_text(value)
                };
                let (min_frac, max_frac, rounding) = if done {
                    let digits = self.fractional_digits.unwrap_or(9);
                    (self.fractional_digits.unwrap_or(0), digits, "trunc")
                } else {
                    (0, 0, "halfExpand")
                };
                let mut parts = format_unit_number(
                    self,
                    name,
                    &unit.style,
                    &raw,
                    sign_display,
                    UnitFraction {
                        min: min_frac,
                        max: max_frac,
                        rounding,
                    },
                )?;
                if need_separator {
                    if let Some(last) = groups.last_mut() {
                        last.push(FormatPart {
                            type_name: "literal".into(),
                            value: ":".into(),
                            source: None,
                            unit: None,
                        });
                        last.append(&mut parts);
                    }
                } else {
                    if matches!(unit.style.as_str(), "numeric" | "2-digit" | "fractional") {
                        need_separator = true;
                    }
                    groups.push(parts);
                }
            }
            if done {
                break;
            }
        }
        flatten_list(&self.locale, &self.style, groups)
    }

    fn unit(&self, index: usize) -> DurationUnitSpec {
        self.units.get(index).cloned().unwrap_or(DurationUnitSpec {
            style: "short".into(),
            display: "auto".into(),
        })
    }
}

fn flatten_list(
    locale: &str,
    style: &str,
    groups: Vec<Vec<FormatPart>>,
) -> Result<Vec<FormatPart>, String> {
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let list_style = if style == "digital" { "short" } else { style };
    let formatter = OwnedListFormatter::try_new(locale, "unit", list_style)?;
    let strings: Vec<String> = groups
        .iter()
        .map(|parts| parts.iter().map(|part| part.value.as_str()).collect())
        .collect();
    let refs: Vec<&str> = strings.iter().map(String::as_str).collect();
    let mut remaining = groups.into_iter();
    let mut flattened = Vec::new();
    for part in formatter.format_parts(&refs)? {
        if part.type_name == "element" {
            if let Some(group) = remaining.next() {
                flattened.extend(group);
            }
        } else {
            flattened.push(part);
        }
    }
    Ok(flattened)
}

struct UnitFraction<'a> {
    min: u32,
    max: u32,
    rounding: &'a str,
}

fn format_unit_number(
    spec: &DurationFormatSpec,
    name: &str,
    style: &str,
    raw: &str,
    sign_display: &str,
    fraction: UnitFraction<'_>,
) -> Result<Vec<FormatPart>, String> {
    let singular = name.trim_end_matches('s');
    let numeric = matches!(style, "numeric" | "2-digit" | "fractional");
    let nf = OwnedNumberFormatter::try_new(NumberFormatSpec {
        locale: spec.locale.clone(),
        numbering_system: spec.numbering_system.clone(),
        style: if numeric {
            "decimal".into()
        } else {
            "unit".into()
        },
        currency: None,
        currency_display: "symbol".into(),
        currency_sign: "standard".into(),
        unit: (!numeric).then(|| singular.to_owned()),
        unit_display: if numeric {
            "short".into()
        } else {
            style.into()
        },
        notation: "standard".into(),
        compact_display: "short".into(),
        sign_display: sign_display.into(),
        use_grouping: if numeric {
            "false".into()
        } else {
            "auto".into()
        },
        minimum_integer_digits: if style == "2-digit" { 2 } else { 1 },
        minimum_fraction_digits: fraction.min,
        maximum_fraction_digits: fraction.max,
        minimum_significant_digits: None,
        maximum_significant_digits: None,
        rounding_mode: fraction.rounding.into(),
        rounding_increment: 1,
        rounding_priority: "auto".into(),
        trailing_zero_display: "auto".into(),
    })?;
    Ok(nf
        .format_parts_str(raw)?
        .into_iter()
        .map(|mut part| {
            part.unit = Some(singular.to_owned());
            part
        })
        .collect())
}

fn duration_to_fractional(values: &[f64; 10], exponent: u32) -> String {
    let to_i128 = |value: f64| format!("{value:.0}").parse::<i128>().unwrap_or(0);
    let seconds = to_i128(values[6]);
    let milliseconds = to_i128(values[7]);
    let microseconds = to_i128(values[8]);
    let nanoseconds = to_i128(values[9]);
    let mut ns = nanoseconds;
    if exponent >= 3 {
        ns += microseconds * 1_000;
    }
    if exponent >= 6 {
        ns += milliseconds * 1_000_000;
    }
    if exponent >= 9 {
        ns += seconds * 1_000_000_000;
    }
    let scale = 10i128.pow(exponent);
    let quot = ns / scale;
    let mut rem = ns % scale;
    if rem < 0 {
        rem = -rem;
    }
    format!("{quot}.{rem:0width$}", width = exponent as usize)
}

fn is_zero_decimal(text: &str) -> bool {
    let digits: String = text.chars().filter(|ch| ch.is_ascii_digit()).collect();
    !digits.is_empty() && digits.chars().all(|ch| ch == '0')
}

fn decimal_text(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() { "-0" } else { "0" }.into();
    }
    format!("{value}")
}

/// Temporal / ISO 8601 持续时间字符串。
pub fn parse_iso_duration(input: &str) -> Result<[f64; 10], String> {
    let (negative, rest) = match input.as_bytes().first() {
        Some(b'+') => (false, &input[1..]),
        Some(b'-') => (true, &input[1..]),
        _ => (false, input),
    };
    let rest = rest
        .strip_prefix('P')
        .ok_or_else(|| "invalid duration".to_owned())?;
    if rest.is_empty() {
        return Err("invalid duration".into());
    }
    let mut fields = [0f64; 10];
    let (date, time) = rest
        .split_once('T')
        .map_or((rest, None), |(date, time)| (date, Some(time)));
    parse_date_section(date, &mut fields)?;
    if let Some(time) = time {
        parse_time_section(time, &mut fields)?;
    }
    if negative {
        for value in &mut fields {
            *value = -*value;
        }
    }
    Ok(fields)
}

fn parse_date_section(input: &str, fields: &mut [f64; 10]) -> Result<(), String> {
    let mut rest = input;
    while !rest.is_empty() {
        let (number, next) = take_number(rest)?;
        let marker = next
            .chars()
            .next()
            .ok_or_else(|| "invalid duration".to_owned())?;
        let index = match marker {
            'Y' => 0,
            'M' => 1,
            'W' => 2,
            'D' => 3,
            _ => return Err("invalid duration".into()),
        };
        fields[index] = number;
        rest = &next[marker.len_utf8()..];
    }
    Ok(())
}

fn parse_time_section(input: &str, fields: &mut [f64; 10]) -> Result<(), String> {
    if input.is_empty() {
        return Err("invalid duration".into());
    }
    let mut rest = input;
    while !rest.is_empty() {
        let (number, next) = take_number(rest)?;
        let marker = next
            .chars()
            .next()
            .ok_or_else(|| "invalid duration".to_owned())?;
        match marker {
            'H' => fields[4] = number,
            'M' => fields[5] = number,
            'S' => split_seconds(number, fields),
            _ => return Err("invalid duration".into()),
        }
        rest = &next[marker.len_utf8()..];
    }
    Ok(())
}

fn split_seconds(value: f64, fields: &mut [f64; 10]) {
    let sign = if value.is_sign_negative() { -1.0 } else { 1.0 };
    let abs = value.abs();
    let seconds = abs.trunc();
    let frac = ((abs - seconds) * 1_000_000_000.0).round() as u64;
    fields[6] = sign * seconds;
    fields[7] = sign * (frac / 1_000_000) as f64;
    fields[8] = sign * ((frac / 1_000) % 1_000) as f64;
    fields[9] = sign * (frac % 1_000) as f64;
}

fn take_number(input: &str) -> Result<(f64, &str), String> {
    let end = input
        .find(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .unwrap_or(input.len());
    if end == 0 {
        return Err("invalid duration".into());
    }
    let number = input[..end]
        .parse::<f64>()
        .map_err(|_| "invalid duration".to_owned())?;
    Ok((number, &input[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_iso_example() {
        let fields = parse_iso_duration("P1Y2M3W4DT5H6M7.00800901S").expect("parse");
        assert_eq!(fields[0], 1.0);
        assert_eq!(fields[6], 7.0);
        assert_eq!(fields[7], 8.0);
        assert_eq!(fields[8], 9.0);
        assert_eq!(fields[9], 10.0);
    }
}
