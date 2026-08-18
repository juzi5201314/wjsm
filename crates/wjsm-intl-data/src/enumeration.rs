//! ECMA-402 `Intl.supportedValuesOf` 的可用值列表。
//!
//! 日历 / 编号系统 / 单位取规范与 CLDR 契约；时区来自 ICU4X IANA 解析器。
//! 排序与去重在此完成，host 只负责包装成 JS 数组。

use std::sync::OnceLock;

use icu::time::zone::iana::IanaParserExtended;

const CALENDARS: &[&str] = &[
    "buddhist",
    "chinese",
    "coptic",
    "dangi",
    "ethioaa",
    "ethiopic",
    "gregory",
    "hebrew",
    "indian",
    "islamic-civil",
    "islamic-tbla",
    "islamic-umalqura",
    "iso8601",
    "japanese",
    "persian",
    "roc",
];

/// 不含规范禁止暴露的 `standard` / `search`。
const COLLATIONS: &[&str] = &[
    "compat", "dict", "emoji", "eor", "phonebk", "phonetic", "pinyin", "reformed", "searchjl",
    "stroke", "trad", "unihan", "zhuyin",
];

const NUMBERING_SYSTEMS: &[&str] = &[
    "adlm", "ahom", "arab", "arabext", "bali", "beng", "bhks", "brah", "cakm", "cham", "deva",
    "diak", "fullwide", "gara", "gong", "gonm", "gujr", "gukh", "guru", "hanidec", "hmng", "hmnp",
    "java", "kali", "kawi", "khmr", "knda", "krai", "lana", "lanatham", "laoo", "latn", "lepc",
    "limb", "mathbold", "mathdbl", "mathmono", "mathsanb", "mathsans", "mlym", "modi", "mong",
    "mroo", "mtei", "mymr", "mymrepka", "mymrpao", "mymrshan", "mymrtlng", "nagm", "newa", "nkoo",
    "olck", "onao", "orya", "osma", "outlined", "rohg", "saur", "segment", "shrd", "sind", "sinh",
    "sora", "sund", "sunu", "takr", "talu", "tamldec", "telu", "thai", "tibt", "tirh", "tnsa",
    "tols", "vaii", "wara", "wcho",
];

const UNITS: &[&str] = &[
    "acre",
    "bit",
    "byte",
    "celsius",
    "centimeter",
    "day",
    "degree",
    "fahrenheit",
    "fluid-ounce",
    "foot",
    "gallon",
    "gigabit",
    "gigabyte",
    "gram",
    "hectare",
    "hour",
    "inch",
    "kilobit",
    "kilobyte",
    "kilogram",
    "kilometer",
    "liter",
    "megabit",
    "megabyte",
    "meter",
    "microsecond",
    "mile",
    "mile-scandinavian",
    "milliliter",
    "millimeter",
    "millisecond",
    "minute",
    "month",
    "nanosecond",
    "ounce",
    "percent",
    "petabyte",
    "pound",
    "second",
    "stone",
    "terabit",
    "terabyte",
    "week",
    "yard",
    "year",
];

/// ISO 4217 常用货币；须为大写三位字母、已排序。
const CURRENCIES: &[&str] = &[
    "AED", "AFN", "ALL", "AMD", "ANG", "AOA", "ARS", "AUD", "AWG", "AZN", "BAM", "BBD", "BDT",
    "BGN", "BHD", "BIF", "BMD", "BND", "BOB", "BRL", "BSD", "BTN", "BWP", "BYN", "BZD", "CAD",
    "CDF", "CHF", "CLP", "CNY", "COP", "CRC", "CUP", "CVE", "CZK", "DJF", "DKK", "DOP", "DZD",
    "EGP", "ERN", "ETB", "EUR", "FJD", "FKP", "GBP", "GEL", "GHS", "GIP", "GMD", "GNF", "GTQ",
    "GYD", "HKD", "HNL", "HRK", "HTG", "HUF", "IDR", "ILS", "INR", "IQD", "IRR", "ISK", "JMD",
    "JOD", "JPY", "KES", "KGS", "KHR", "KMF", "KPW", "KRW", "KWD", "KYD", "KZT", "LAK", "LBP",
    "LKR", "LRD", "LSL", "LYD", "MAD", "MDL", "MGA", "MKD", "MMK", "MNT", "MOP", "MRU", "MUR",
    "MVR", "MWK", "MXN", "MYR", "MZN", "NAD", "NGN", "NIO", "NOK", "NPR", "NZD", "OMR", "PAB",
    "PEN", "PGK", "PHP", "PKR", "PLN", "PYG", "QAR", "RON", "RSD", "RUB", "RWF", "SAR", "SBD",
    "SCR", "SDG", "SEK", "SGD", "SHP", "SLE", "SOS", "SRD", "SSP", "STN", "SYP", "SZL", "THB",
    "TJS", "TMT", "TND", "TOP", "TRY", "TTD", "TWD", "TZS", "UAH", "UGX", "USD", "UYU", "UZS",
    "VES", "VND", "VUV", "WST", "XAF", "XCD", "XOF", "XPF", "YER", "ZAR", "ZMW", "ZWG",
];

pub fn available_calendars() -> &'static [&'static str] {
    CALENDARS
}

pub fn available_collations() -> &'static [&'static str] {
    COLLATIONS
}

/// 某 locale 是否支持该 collation。`standard` / `search` 规范禁止出现在 resolvedOptions。
pub fn collation_supported(locale: &str, collation: &str) -> bool {
    if matches!(collation, "standard" | "search") {
        return false;
    }
    if !COLLATIONS.contains(&collation) {
        return false;
    }
    language_collations(locale_language(locale)).contains(&collation)
}

/// Thai 默认忽略标点（CLDR `colAlternate=shifted`）。
pub fn default_ignore_punctuation(locale: &str) -> bool {
    locale_language(locale) == "th"
}

fn locale_language(locale: &str) -> &str {
    locale.split(['-', '_']).next().unwrap_or(locale).trim()
}

fn language_collations(language: &str) -> &'static [&'static str] {
    match language {
        "de" | "fi" => &["eor", "phonebk"],
        "zh" => &["eor", "pinyin", "stroke", "unihan", "zhuyin"],
        "ko" => &["eor", "searchjl", "unihan"],
        "sv" => &["eor", "reformed"],
        "es" => &["eor", "trad"],
        "si" => &["dict", "eor"],
        "ar" => &["compat", "eor"],
        "ln" => &["eor", "phonetic"],
        _ => &["emoji", "eor"],
    }
}

pub fn available_numbering_systems() -> &'static [&'static str] {
    NUMBERING_SYSTEMS
}

pub fn available_units() -> &'static [&'static str] {
    UNITS
}

/// ECMA-402 well-formed unit identifier：sanctioned simple 或 `simple-per-simple`。
pub fn is_well_formed_unit_identifier(unit: &str) -> bool {
    if UNITS.contains(&unit) {
        return true;
    }
    let Some((numerator, denominator)) = unit.split_once("-per-") else {
        return false;
    };
    UNITS.contains(&numerator) && UNITS.contains(&denominator)
}

pub fn available_currencies() -> &'static [&'static str] {
    CURRENCIES
}

pub fn available_time_zones() -> &'static [String] {
    static ZONES: OnceLock<Vec<String>> = OnceLock::new();
    ZONES.get_or_init(|| {
        let mut zones = IanaParserExtended::new()
            .iter()
            .map(|item| item.canonical.to_owned())
            .collect::<Vec<_>>();
        // ECMA-402 把 Etc/UTC、Etc/GMT 规范成 UTC，不能出现在 AvailableTimeZones 里。
        zones.retain(|zone| zone != "Etc/UTC" && zone != "Etc/GMT");
        if !zones.iter().any(|zone| zone == "UTC") {
            zones.push("UTC".to_owned());
        }
        zones.sort();
        zones.dedup();
        zones
    })
}

/// CanonicalizeTimeZoneName：UTC 大小写折叠、偏移补全、IANA 大小写不敏感。
pub fn canonicalize_time_zone(name: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("Invalid time zone".into());
    }
    if name.eq_ignore_ascii_case("utc")
        || name.eq_ignore_ascii_case("etc/utc")
        || name.eq_ignore_ascii_case("etc/gmt")
    {
        return Ok("UTC".into());
    }
    if let Some(offset) = canonicalize_offset(name) {
        return Ok(offset);
    }
    available_time_zones()
        .iter()
        .find(|zone| zone.eq_ignore_ascii_case(name))
        .cloned()
        .ok_or_else(|| format!("Invalid time zone {name}"))
}

fn canonicalize_offset(name: &str) -> Option<String> {
    // U+2212 MINUS SIGN 不是合法 offset 时区标识。
    let rest = name.strip_prefix('+').or_else(|| name.strip_prefix('-'))?;
    let (hours, minutes) = if rest.len() == 2 && rest.bytes().all(|b| b.is_ascii_digit()) {
        (rest, "00")
    } else if rest.len() == 4 && rest.bytes().all(|b| b.is_ascii_digit()) {
        (&rest[..2], &rest[2..])
    } else if rest.len() == 5 && rest.as_bytes().get(2) == Some(&b':') {
        let hours = &rest[..2];
        let minutes = &rest[3..];
        if hours.bytes().all(|b| b.is_ascii_digit()) && minutes.bytes().all(|b| b.is_ascii_digit())
        {
            (hours, minutes)
        } else {
            return None;
        }
    } else {
        return None;
    };
    let hour: u8 = hours.parse().ok()?;
    let minute: u8 = minutes.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    if hour == 0 && minute == 0 {
        return Some("+00:00".into());
    }
    let sign = if name.starts_with('+') { '+' } else { '-' };
    Some(format!("{sign}{hour:02}:{minute:02}"))
}

pub fn supported_values(key: &str) -> Result<Vec<String>, String> {
    Ok(match key {
        "calendar" => available_calendars()
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        "collation" => available_collations()
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        "currency" => available_currencies()
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        "numberingSystem" => available_numbering_systems()
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        "timeZone" => available_time_zones().to_vec(),
        "unit" => available_units()
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        _ => return Err(format!("Invalid key: {key}")),
    })
}

#[cfg(test)]
mod tests {
    use super::{available_calendars, available_time_zones, supported_values};

    #[test]
    fn calendars_include_gregory_and_are_sorted() {
        let calendars = available_calendars();
        assert!(calendars.contains(&"gregory"));
        let mut sorted = calendars.to_vec();
        sorted.sort_unstable();
        assert_eq!(calendars, sorted.as_slice());
    }

    #[test]
    fn time_zones_include_non_continental() {
        let zones = available_time_zones();
        assert!(zones.iter().any(|zone| zone == "UTC" || zone == "Etc/UTC"));
    }

    #[test]
    fn rejects_unknown_key() {
        assert!(supported_values("hourCycle").is_err());
    }

    #[test]
    fn german_phonebook_is_supported_pinyin_is_not() {
        assert!(super::collation_supported("de", "phonebk"));
        assert!(super::collation_supported("de", "eor"));
        assert!(!super::collation_supported("de", "pinyin"));
        assert!(!super::collation_supported("en", "phonebk"));
        assert!(!super::collation_supported("en", "pinyin"));
        assert!(!super::collation_supported("de", "standard"));
        assert!(!super::collation_supported("de", "search"));
    }

    #[test]
    fn thai_defaults_to_ignore_punctuation() {
        assert!(super::default_ignore_punctuation("th"));
        assert!(super::default_ignore_punctuation("th-TH"));
        assert!(!super::default_ignore_punctuation("en"));
        assert!(!super::default_ignore_punctuation("ja"));
    }
}
