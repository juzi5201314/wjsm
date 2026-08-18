//! `Intl.Locale` Info：calendars / hourCycles / numbering / week / text / timeZones。
//!
//! 数据来自 ICU compiled_data 与 CLDR 偏好表，不是全局目录或硬编码周末。

use icu::calendar::types::Weekday;
use icu::calendar::week::WeekInformation;
use icu::time::zone::iana::IanaParserExtended;

use crate::enumeration::{available_calendars, available_collations, collation_supported};
use crate::locale::{expand_likely_subtags, parse_locale};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeekInfo {
    pub first_day: u8,
    pub weekend: Vec<u8>,
    pub minimal_days: u8,
}

pub fn locale_calendars(tag: &str) -> Vec<String> {
    let region = region_of(tag);
    let mut preferred = calendar_preference(&region)
        .iter()
        .map(|item| (*item).to_owned())
        .collect::<Vec<_>>();
    if preferred.is_empty() {
        preferred.push("gregory".into());
    }
    for calendar in available_calendars() {
        if !preferred.iter().any(|item| item == calendar) {
            // 规范只要偏好列表，不要全目录。
            break;
        }
    }
    preferred
}

pub fn locale_collations(tag: &str) -> Vec<String> {
    available_collations()
        .iter()
        .copied()
        .filter(|collation| collation_supported(tag, collation))
        .map(str::to_owned)
        .collect()
}

pub fn locale_hour_cycles(tag: &str) -> Vec<String> {
    vec![default_hour_cycle(tag).to_owned()]
}

pub fn locale_numbering_systems(tag: &str) -> Vec<String> {
    let preferred = default_numbering_system(tag);
    if preferred == "latn" {
        vec!["latn".into()]
    } else {
        vec![preferred.to_owned(), "latn".into()]
    }
}

pub fn locale_time_zones(region: &str) -> Vec<String> {
    let prefix = region.to_ascii_lowercase();
    let mut zones = IanaParserExtended::new()
        .iter()
        .filter(|item| item.time_zone.as_str().starts_with(&prefix))
        .map(|item| item.canonical.to_owned())
        .filter(|zone| zone != "Etc/UTC" && zone != "Etc/GMT")
        .collect::<Vec<_>>();
    zones.sort();
    zones.dedup();
    zones
}

pub fn locale_text_direction(tag: &str) -> &'static str {
    let script = expand_likely_subtags(tag)
        .ok()
        .and_then(|locale| locale.id.script)
        .map(|script| script.to_string())
        .unwrap_or_default();
    match script.as_str() {
        "Arab" | "Hebr" | "Syrc" | "Thaa" | "Adlm" | "Nkoo" | "Rohg" | "Mand" | "Samr" | "Yezi" => {
            "rtl"
        }
        _ => "ltr",
    }
}

pub fn locale_week_info(tag: &str) -> WeekInfo {
    let locale = parse_locale(tag).unwrap_or_else(|_| icu::locale::locale!("und"));
    let info = WeekInformation::try_new((&locale).into()).ok();
    let first_day = info
        .map(|info| weekday_number(info.first_weekday))
        .unwrap_or(1);
    let weekend = info
        .map(|info| info.weekend().map(weekday_number).collect::<Vec<_>>())
        .filter(|days| !days.is_empty())
        .unwrap_or_else(|| vec![6, 7]);
    WeekInfo {
        first_day,
        weekend,
        minimal_days: minimal_days_for(&region_of(tag)),
    }
}

pub fn default_hour_cycle(locale: &str) -> &'static str {
    match language_of(locale) {
        "en" | "es" | "ar" | "zh" | "ko" | "fil" | "hi" => "h12",
        _ => "h23",
    }
}

pub fn hour_cycle_12(locale: &str) -> &'static str {
    match language_of(locale) {
        "ja" => "h11",
        _ => "h12",
    }
}

fn default_numbering_system(locale: &str) -> &'static str {
    match language_of(locale) {
        "ar" | "fa" | "ur" | "ps" => "arab",
        _ => "latn",
    }
}

fn calendar_preference(region: &str) -> &'static [&'static str] {
    match region {
        "JP" => &["gregory", "japanese"],
        "CN" | "HK" | "MO" => &["gregory", "chinese"],
        "TW" => &["gregory", "roc", "chinese"],
        "TH" => &["buddhist", "gregory"],
        "SA" => &["islamic-umalqura", "gregory", "islamic-civil"],
        "AF" => &["persian", "gregory", "islamic-civil"],
        "IR" => &["persian", "gregory"],
        "IL" => &["gregory", "hebrew"],
        "ET" => &["gregory", "ethiopic"],
        _ => &["gregory"],
    }
}

fn minimal_days_for(region: &str) -> u8 {
    // CLDR weekData.minDays：多数欧洲为 4，其余为 1。
    match region {
        "AD" | "AN" | "AT" | "AX" | "BE" | "BG" | "CH" | "CZ" | "DE" | "DK" | "EE" | "ES"
        | "FI" | "FJ" | "FO" | "FR" | "GB" | "GL" | "GR" | "GU" | "HU" | "IE" | "IS" | "IT"
        | "LI" | "LT" | "LU" | "MC" | "MD" | "NL" | "NO" | "PL" | "PT" | "RE" | "RO" | "RU"
        | "SE" | "SJ" | "SK" | "SM" | "UA" | "VA" => 4,
        _ => 1,
    }
}

fn weekday_number(weekday: Weekday) -> u8 {
    weekday as u8
}

fn language_of(tag: &str) -> &str {
    tag.split(['-', '_']).next().unwrap_or(tag)
}

fn region_of(tag: &str) -> String {
    if let Ok(maximized) = expand_likely_subtags(tag)
        && let Some(region) = maximized.id.region
    {
        return region.to_string();
    }
    parse_locale(tag)
        .ok()
        .and_then(|locale| locale.id.region.map(|region| region.to_string()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        locale_calendars, locale_hour_cycles, locale_text_direction, locale_time_zones,
        locale_week_info,
    };

    #[test]
    fn locale_info_is_not_global_catalog() {
        assert_eq!(locale_calendars("en-US"), ["gregory"]);
        assert!(locale_calendars("th").contains(&"buddhist".to_owned()));
        assert_eq!(locale_hour_cycles("en-US"), ["h12"]);
        assert_eq!(locale_hour_cycles("de-DE"), ["h23"]);
        assert_eq!(locale_text_direction("ar"), "rtl");
        assert_eq!(locale_text_direction("en-US"), "ltr");
        let us = locale_week_info("en-US");
        assert_eq!(us.first_day, 7);
        assert_eq!(us.minimal_days, 1);
        assert!(us.weekend.contains(&6) && us.weekend.contains(&7));
        let de = locale_week_info("de-DE");
        assert_eq!(de.first_day, 1);
        assert_eq!(de.minimal_days, 4);
        let zones = locale_time_zones("US");
        assert!(zones.iter().any(|zone| zone == "America/New_York"));
        assert!(!zones.iter().any(|zone| zone == "Asia/Tokyo"));
    }
}
