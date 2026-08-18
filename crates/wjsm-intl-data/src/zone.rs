//! 默认时区、IANA 墙钟换算与时区显示名。
//!
//! IANA 标识来自 ICU compiled_data。DST 墙钟换算由本 crate 拥有，
//! host-native 不得再依赖第二套 tzdb。

use std::path::Path;
use std::sync::OnceLock;

use chrono::{Offset, TimeZone as ChronoTimeZone, Utc};
use chrono_tz::Tz;

use crate::enumeration::canonicalize_time_zone;

/// ECMA-402 `DefaultTimeZone`：宿主 IANA 标识。
pub fn default_time_zone() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED.get_or_init(detect_default_time_zone).clone()
}

pub fn detect_default_time_zone() -> String {
    if let Ok(tz) = std::env::var("TZ")
        && let Some(name) = timezone_from_tz_value(&tz)
    {
        return name;
    }
    if let Ok(link) = std::fs::read_link("/etc/localtime")
        && let Some(name) = timezone_from_path(&link)
    {
        return name;
    }
    "UTC".into()
}

/// `TZ` 环境值：`America/New_York`、`:/usr/share/zoneinfo/Asia/Shanghai`。
pub fn timezone_from_tz_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "UTC" || trimmed == "utc" {
        return Some("UTC".into());
    }
    let stripped = trimmed.trim_start_matches(':');
    if let Ok(name) = canonicalize_time_zone(stripped) {
        return Some(name);
    }
    timezone_from_path(Path::new(stripped))
}

pub fn timezone_from_path(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    let rest = text.split("zoneinfo/").nth(1)?;
    canonicalize_time_zone(rest).ok()
}

/// 构造器在 ICU 认可该 IANA 名之后，确认本 crate 能做墙钟换算。
pub fn ensure_zone_convertible(time_zone: &str) -> Result<(), String> {
    if time_zone == "UTC" || offset_minutes(time_zone).is_some() {
        return Ok(());
    }
    time_zone
        .parse::<Tz>()
        .map(|_| ())
        .map_err(|_| format!("Invalid time zone {time_zone}"))
}

/// UTC 瞬时 → 该区墙钟瞬时（仍用 Unix millis 编码，便于 ICU 当 naive datetime）。
pub fn utc_to_wall_millis(millis: f64, time_zone: &str) -> Result<f64, String> {
    if time_zone == "UTC" {
        return Ok(millis);
    }
    if let Some(minutes) = offset_minutes(time_zone) {
        return Ok(millis + f64::from(minutes) * 60_000.0);
    }
    let tz: Tz = time_zone
        .parse()
        .map_err(|_| format!("Invalid time zone {time_zone}"))?;
    Ok(wall_millis(millis, tz))
}

pub fn utc_offset_seconds(millis: f64, time_zone: &str) -> Result<i32, String> {
    if time_zone == "UTC" {
        return Ok(0);
    }
    if let Some(minutes) = offset_minutes(time_zone) {
        return Ok(minutes * 60);
    }
    let tz: Tz = time_zone
        .parse()
        .map_err(|_| format!("Invalid time zone {time_zone}"))?;
    let Some(local) = tz.timestamp_millis_opt(millis as i64).single() else {
        return Err(format!("Invalid time zone {time_zone}"));
    };
    Ok(local.offset().fix().local_minus_utc())
}

pub fn time_zone_display_name(time_zone: &str, style: &str, millis: f64) -> Result<String, String> {
    let seconds = utc_offset_seconds(millis, time_zone)?;
    match style {
        "shortOffset" => Ok(offset_label(seconds, false)),
        "longOffset" => Ok(offset_label(seconds, true)),
        "short" | "shortGeneric" => Ok(short_zone_name(time_zone, seconds)),
        _ => Ok(long_zone_name(time_zone, seconds)),
    }
}

fn wall_millis(millis: f64, tz: Tz) -> f64 {
    let Some(local) = tz.timestamp_millis_opt(millis as i64).single() else {
        return millis;
    };
    use chrono::{Datelike, Timelike};
    let millis_of_second = f64::from(local.nanosecond() / 1_000_000);
    Utc.with_ymd_and_hms(
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        local.second(),
    )
    .single()
    .map(|utc| utc.timestamp_millis() as f64 + millis_of_second)
    .unwrap_or(millis)
}

fn offset_minutes(time_zone: &str) -> Option<i32> {
    if let Some(rest) = time_zone
        .strip_prefix("Etc/GMT")
        .or_else(|| time_zone.strip_prefix("etc/gmt"))
    {
        if rest.is_empty() {
            return Some(0);
        }
        let hours: i32 = rest.parse().ok()?;
        return hours.checked_mul(-60);
    }
    let rest = time_zone
        .strip_prefix('+')
        .or_else(|| time_zone.strip_prefix('-'))?;
    let (hours, minutes) = rest.split_once(':')?;
    let hours: i32 = hours.parse().ok()?;
    let minutes: i32 = minutes.parse().ok()?;
    let total = hours.checked_mul(60)?.checked_add(minutes)?;
    Some(if time_zone.starts_with('-') {
        -total
    } else {
        total
    })
}

fn offset_label(seconds: i32, long: bool) -> String {
    if seconds == 0 {
        return if long { "GMT".into() } else { "GMT".into() };
    }
    let sign = if seconds >= 0 { '+' } else { '-' };
    let abs = seconds.unsigned_abs();
    let hours = abs / 3600;
    let minutes = (abs % 3600) / 60;
    if long || minutes != 0 {
        format!("GMT{sign}{hours:02}:{minutes:02}")
    } else {
        format!("GMT{sign}{hours}")
    }
}

fn short_zone_name(time_zone: &str, seconds: i32) -> String {
    if time_zone == "UTC" {
        return "UTC".into();
    }
    offset_label(seconds, false)
}

fn long_zone_name(time_zone: &str, seconds: i32) -> String {
    if time_zone == "UTC" {
        return "Coordinated Universal Time".into();
    }
    if let Some(city) = time_zone.rsplit('/').next() {
        let city = city.replace('_', " ");
        return format!("{city} ({})", offset_label(seconds, true));
    }
    offset_label(seconds, true)
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_zone_convertible, timezone_from_path, timezone_from_tz_value, utc_to_wall_millis,
    };
    use chrono::{TimeZone, Timelike, Utc};
    use std::path::Path;

    fn utc_millis(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> f64 {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("utc instant")
            .timestamp_millis() as f64
    }

    fn wall_hour(millis: f64) -> u32 {
        Utc.timestamp_millis_opt(millis as i64)
            .single()
            .expect("wall instant")
            .hour()
    }

    #[test]
    fn tz_env_and_zoneinfo_path_canonicalize() {
        assert_eq!(
            timezone_from_tz_value("America/New_York").as_deref(),
            Some("America/New_York")
        );
        assert_eq!(
            timezone_from_path(Path::new("/usr/share/zoneinfo/Asia/Shanghai")).as_deref(),
            Some("Asia/Shanghai")
        );
    }

    #[test]
    fn iana_wall_clock_and_dst() {
        let winter = utc_millis(2024, 1, 15, 17, 0);
        assert_eq!(
            wall_hour(utc_to_wall_millis(winter, "America/New_York").unwrap()),
            12
        );
        assert_eq!(
            wall_hour(utc_to_wall_millis(winter, "Asia/Tokyo").unwrap()),
            2
        );
        let summer = utc_millis(2024, 7, 15, 16, 0);
        assert_eq!(
            wall_hour(utc_to_wall_millis(summer, "America/New_York").unwrap()),
            12
        );
    }

    #[test]
    fn unknown_zone_fails_closed() {
        assert!(ensure_zone_convertible("Not/AZone").is_err());
        assert!(utc_to_wall_millis(0.0, "Not/AZone").is_err());
    }
}
