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
