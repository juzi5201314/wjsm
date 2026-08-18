//! Date 纯解析 / 时区转换（不依赖任何具体执行后端）。

use chrono::{DateTime, Local, TimeZone, Utc};

/// ES-style floor division on f64 (quotient toward −∞).
fn f64_euclid_div(ms: f64, divisor: f64) -> f64 {
    (ms / divisor).floor()
}

/// Non-negative remainder: `ms - divisor * f64_euclid_div(ms, divisor)` (like `rem_euclid`).
fn f64_euclid_rem(ms: f64, divisor: f64) -> f64 {
    ms - divisor * f64_euclid_div(ms, divisor)
}

/// Milliseconds within the current second (0 ≤ m < 1000), per ES `mod` / floor division.
fn ms_within_second(ms: f64) -> f64 {
    f64_euclid_rem(ms, 1000.0)
}

/// Whole-second part of a time value using floor division (not trunc toward zero).
fn floor_ms_to_secs(ms: f64) -> i64 {
    f64_euclid_div(ms, 1000.0) as i64
}

/// UTC 年（含 chrono 无法表示的 ES 时间边界）。
pub fn utc_year_from_ms(ms: f64) -> Option<i32> {
    if !ms.is_finite() || ms.abs() > 8.64e15 {
        return None;
    }
    let days = (ms / 86_400_000.0).floor() as i64;
    Some(civil_from_days(days).0)
}

/// Howard Hinnant `civil_from_days`：Unix 纪元日 → 格里高利年月日。
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z.saturating_sub(era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month as u32, day as u32)
}

/// 毫秒时间戳 → UTC DateTime。
pub fn ms_to_datetime_utc(ms: f64) -> Option<DateTime<Utc>> {
    if ms.is_nan() || ms.is_infinite() {
        return None;
    }
    let secs = floor_ms_to_secs(ms);
    let sub_ms = ms_within_second(ms);
    if !(0.0..1000.0).contains(&sub_ms) {
        return None;
    }
    let nanos = (sub_ms * 1_000_000.0).round() as u32;
    Utc.timestamp_opt(secs, nanos).single()
}

/// 毫秒时间戳 → 本地时区 DateTime。
pub fn ms_to_datetime_local(ms: f64) -> Option<DateTime<Local>> {
    if ms.is_nan() || ms.is_infinite() {
        return None;
    }
    let utc_dt = ms_to_datetime_utc(ms)?;
    Some(utc_dt.with_timezone(&Local))
}

/// 按 ECMAScript `Date.parse` / `new Date(string)` 期望解析日期字符串。
/// 支持 ISO 8601 (RFC3339)、常见 chrono 格式、`Date.prototype.toString()` /
/// `Date.prototype.toUTCString()` 输出。
pub fn parse_date_string(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let s = s
        .trim_end()
        .trim_end_matches("(Coordinated Universal Time)")
        .trim();

    if let Some(millis) = parse_es_date_time_string(s) {
        return Some(millis);
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis() as f64);
    }

    const NAIVE_FMTS: &[&str] = &[
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d",
        "%b %d, %Y",
        "%B %d, %Y",
        "%d %b %Y %H:%M:%S",
    ];
    for fmt in NAIVE_FMTS {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(ndt.and_utc().timestamp_millis() as f64);
        }
        if let Ok(nd) = chrono::NaiveDate::parse_from_str(s, fmt)
            && let Some(ndt) = nd.and_hms_opt(0, 0, 0)
        {
            return Some(ndt.and_utc().timestamp_millis() as f64);
        }
    }

    // `Date.prototype.toUTCString()`: "Wed, 22 Jun 2026 12:00:00 GMT"
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, "%a, %d %b %Y %H:%M:%S GMT") {
        return Some(ndt.and_utc().timestamp_millis() as f64);
    }

    // `Date.prototype.toString()`: "Tue Jun 22 2026 12:00:00 GMT+0000"
    let gmt_stripped = s.replace("GMT", "");
    let gmt_stripped = gmt_stripped.trim();
    if let Ok(dt) = DateTime::parse_from_str(gmt_stripped, "%a %b %e %Y %H:%M:%S %z") {
        return Some(dt.timestamp_millis() as f64);
    }
    if let Ok(dt) = DateTime::parse_from_str(gmt_stripped, "%a %b %e %Y %H:%M:%S %:z") {
        return Some(dt.timestamp_millis() as f64);
    }

    None
}

/// ECMA-262 Date Time String Format：`YYYY-MM-DDTHH:mm[:ss[.sss]][Z|±HH:mm]`。
fn parse_es_date_time_string(s: &str) -> Option<f64> {
    if s.len() < 4 || !s.as_bytes()[..4].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let (year, month, day, rest) = parse_es_date(s)?;
    if rest.is_empty() {
        return utc_millis(year, month, day, 0, 0, 0, 0);
    }
    let rest = rest.strip_prefix('T').or_else(|| rest.strip_prefix('t'))?;
    let (hour, minute, second, millis, offset) = parse_es_time(rest)?;
    let utc = utc_millis(year, month, day, hour, minute, second, millis)?;
    match offset {
        Some(minutes) => Some(utc - f64::from(minutes) * 60_000.0),
        None => local_millis(year, month, day, hour, minute, second, millis),
    }
}

fn parse_es_date(s: &str) -> Option<(i32, u32, u32, &str)> {
    let year = s.get(..4)?.parse().ok()?;
    let rest = &s[4..];
    if !rest.starts_with('-') {
        return Some((year, 1, 1, rest));
    }
    let month = rest.get(1..3)?.parse().ok()?;
    let rest = &rest[3..];
    if !rest.starts_with('-') {
        return Some((year, month, 1, rest));
    }
    let day = rest.get(1..3)?.parse().ok()?;
    Some((year, month, day, &rest[3..]))
}

fn parse_es_time(s: &str) -> Option<(u32, u32, u32, u32, Option<i32>)> {
    let hour = s.get(..2)?.parse().ok()?;
    let rest = s.get(2..)?.strip_prefix(':')?;
    let minute = rest.get(..2)?.parse().ok()?;
    let rest = &rest[2..];
    let (second, millis, rest) = if let Some(rest) = rest.strip_prefix(':') {
        let second = rest.get(..2)?.parse().ok()?;
        let rest = &rest[2..];
        if let Some(rest) = rest.strip_prefix('.') {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            let millis = pad_millis(rest.get(..digits.min(3))?)?;
            (second, millis, &rest[digits..])
        } else {
            (second, 0, rest)
        }
    } else {
        (0, 0, rest)
    };
    Some((hour, minute, second, millis, parse_es_offset(rest)?))
}

fn pad_millis(digits: &str) -> Option<u32> {
    let mut value = digits.parse::<u32>().ok()?;
    for _ in digits.len()..3 {
        value *= 10;
    }
    Some(value)
}

fn parse_es_offset(s: &str) -> Option<Option<i32>> {
    if s.is_empty() {
        return Some(None);
    }
    if s == "Z" || s == "z" {
        return Some(Some(0));
    }
    let (sign, rest) = if let Some(rest) = s.strip_prefix('+') {
        (1i32, rest)
    } else {
        (-1, s.strip_prefix('-')?)
    };
    let (hours, minutes, consumed) = if rest.len() >= 5 && rest.as_bytes().get(2) == Some(&b':') {
        (
            rest.get(..2)?.parse::<i32>().ok()?,
            rest.get(3..5)?.parse::<i32>().ok()?,
            5,
        )
    } else if rest.len() >= 4 {
        (
            rest.get(..2)?.parse::<i32>().ok()?,
            rest.get(2..4)?.parse::<i32>().ok()?,
            4,
        )
    } else {
        return None;
    };
    if rest.len() != consumed {
        return None;
    }
    Some(Some(sign * (hours * 60 + minutes)))
}

fn utc_millis(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millis: u32,
) -> Option<f64> {
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    let time = date.and_hms_milli_opt(hour, minute, second, millis)?;
    Some(time.and_utc().timestamp_millis() as f64)
}

fn local_millis(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millis: u32,
) -> Option<f64> {
    Local
        .with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .map(|dt| dt.timestamp_millis() as f64 + f64::from(millis))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_es_iso_without_seconds() {
        assert_eq!(
            parse_date_string("2000-01-01T00:00Z"),
            Some(946_684_800_000.0)
        );
        assert_eq!(
            parse_date_string("2000-01-01T00:00:00Z"),
            parse_date_string("2000-01-01T00:00Z")
        );
    }

    #[test]
    fn utc_year_covers_es_time_bounds() {
        assert_eq!(utc_year_from_ms(0.0), Some(1970));
        assert_eq!(utc_year_from_ms(-8.64e15), Some(-271_821));
    }
}
