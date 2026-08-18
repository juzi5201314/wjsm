//! `Intl.DateTimeFormat` 的 ICU fieldset 映射与 TimeClip。

use icu::calendar::Gregorian;
use icu::calendar::Iso;
use icu::calendar::types::RataDie;
use icu::datetime::fieldsets::builder::{DateFields, FieldSetBuilder};
use icu::datetime::fieldsets::enums::{CompositeDateTimeFieldSet, DateFieldSet, TimeFieldSet};
use icu::datetime::fieldsets::{T, YMD, YMDE};
use icu::datetime::input::{Date, DateTime, Time};
use icu::datetime::options::{Length, SubsecondDigits, TimePrecision, YearStyle};
use icu::datetime::pattern::{DateTimePattern, DayPeriodNameLength, FixedCalendarDateTimeNames};
use icu::datetime::{DateTimeFormatter, DateTimeFormatterPreferences};
use icu::locale::Locale;
use icu::locale::preferences::extensions::unicode::keywords::HourCycle;
use writeable::TryWriteable;

use crate::format::{FormatPart, collect_parts};

const MS_PER_DAY: f64 = 86_400_000.0;
const TIME_CLIP: f64 = 8.64e15;

#[derive(Clone, Debug)]
pub struct DateTimeFormatSpec {
    pub locale: String,
    pub calendar: String,
    pub numbering_system: String,
    pub hour_cycle: Option<String>,
    pub date_style: Option<String>,
    pub time_style: Option<String>,
    pub weekday: Option<String>,
    pub era: Option<String>,
    pub year: Option<String>,
    pub month: Option<String>,
    pub day: Option<String>,
    pub day_period: Option<String>,
    pub hour: Option<String>,
    pub minute: Option<String>,
    pub second: Option<String>,
    pub fractional_second_digits: Option<u32>,
    pub time_zone: String,
    pub time_zone_name: Option<String>,
}

pub struct OwnedDateTimeFormatter {
    inner: DateTimeFormatter<CompositeDateTimeFieldSet>,
    locale: Locale,
    numbering_system: String,
    day_period: Option<String>,
    date_style: Option<String>,
    time_style: Option<String>,
    time_zone: String,
    hour: Option<String>,
    minute: Option<String>,
    second: Option<String>,
    year: Option<String>,
    calendar: String,
    keep: Option<Vec<&'static str>>,
}

impl OwnedDateTimeFormatter {
    pub fn try_new(spec: &DateTimeFormatSpec) -> Result<Self, String> {
        let locale = format_locale(spec)?;
        let mut prefs = DateTimeFormatterPreferences::from(&locale);
        prefs.hour_cycle = spec.hour_cycle.as_deref().and_then(hour_cycle_pref);
        let fieldset = build_fieldset(spec)?;
        Ok(Self {
            inner: DateTimeFormatter::try_new(prefs, fieldset).map_err(err)?,
            locale,
            numbering_system: spec.numbering_system.clone(),
            day_period: spec.day_period.clone(),
            date_style: spec.date_style.clone(),
            time_style: spec.time_style.clone(),
            time_zone: spec.time_zone.clone(),
            hour: spec.hour.clone(),
            minute: spec.minute.clone(),
            second: spec.second.clone(),
            year: spec.year.clone(),
            calendar: spec.calendar.clone(),
            keep: keep_fields(spec),
        })
    }

    pub fn format_millis(&self, millis: f64) -> Result<String, String> {
        Ok(self
            .format_parts_millis(millis)?
            .into_iter()
            .map(|part| part.value)
            .collect())
    }

    pub fn format_parts_millis(&self, millis: f64) -> Result<Vec<FormatPart>, String> {
        let (days, millis_of_day) = split_days(millis)?;
        let date = Date::from_rata_die(unix_epoch_rd() + days, Iso);
        let time = time_of_day(millis_of_day)?;
        if self.keep.as_deref() == Some(&["dayPeriod"][..])
            && let Some(style) = &self.day_period
        {
            let value = format_day_period(&self.locale, style, time)?;
            return Ok(vec![FormatPart {
                type_name: "dayPeriod".into(),
                value,
                source: None,
                unit: None,
            }]);
        }
        let mut parts = collect_parts(&self.inner.format(&DateTime { date, time }))?;
        let year = date.year().extended_year();
        for part in &mut parts {
            if part.type_name == "fraction" {
                part.type_name = "fractionalSecond".into();
            }
            if matches!(self.calendar.as_str(), "chinese" | "dangi") && part.type_name == "year" {
                part.type_name = "relatedYear".into();
            }
            part.value = remap_numeric(&part.value, &self.numbering_system);
            let zero = numbering_zero(&self.numbering_system);
            pad_two_digit(part, "hour", self.hour.as_deref(), zero);
            pad_two_digit(part, "minute", self.minute.as_deref(), zero);
            pad_two_digit(part, "second", self.second.as_deref(), zero);
        }
        rewrite_year_digits(
            &mut parts,
            year,
            self.date_style.as_deref(),
            self.year.as_deref(),
        );
        for part in &mut parts {
            if part.type_name == "literal" {
                part.value = part.value.replace('\u{202f}', " ");
            }
        }
        if let Some(style) = &self.day_period {
            let hour = time.hour.number();
            for part in &mut parts {
                if part.type_name == "dayPeriod" {
                    part.value = flexible_day_period(hour, style).into();
                }
            }
        }
        if let Some(name) = utc_zone_name(self.time_style.as_deref(), &self.time_zone) {
            if !parts.is_empty() {
                parts.push(FormatPart {
                    type_name: "literal".into(),
                    value: " ".into(),
                    source: None,
                    unit: None,
                });
            }
            parts.push(FormatPart {
                type_name: "timeZoneName".into(),
                value: name.into(),
                source: None,
                unit: None,
            });
        }
        let mut parts = match &self.keep {
            Some(keep) => filter_parts(parts, keep),
            None => parts,
        };
        if self.year.is_some() {
            inject_cyclic_year_name(
                &mut parts,
                date.to_rata_die(),
                &self.calendar,
                &self.locale.to_string(),
            );
        }
        Ok(parts)
    }
}

fn utc_zone_name(time_style: Option<&str>, time_zone: &str) -> Option<&'static str> {
    if time_zone != "UTC" {
        return None;
    }
    match time_style {
        Some("full") => Some("Coordinated Universal Time"),
        Some("long") => Some("UTC"),
        _ => None,
    }
}

fn format_locale(spec: &DateTimeFormatSpec) -> Result<Locale, String> {
    let mut tag = spec.locale.clone();
    let mut keys = Vec::new();
    if spec.calendar != "gregory" {
        keys.push(format!("ca-{}", spec.calendar));
    }
    if spec.numbering_system != "latn" {
        keys.push(format!("nu-{}", spec.numbering_system));
    }
    if let Some(cycle) = &spec.hour_cycle {
        keys.push(format!("hc-{cycle}"));
    }
    if !keys.is_empty() {
        tag = if tag.contains("-u-") {
            format!("{tag}-{}", keys.join("-"))
        } else {
            format!("{tag}-u-{}", keys.join("-"))
        };
    }
    Locale::try_from_str(&tag).map_err(err)
}

fn build_fieldset(spec: &DateTimeFormatSpec) -> Result<CompositeDateTimeFieldSet, String> {
    if let Some(fieldset) = style_fieldset(spec) {
        return Ok(fieldset);
    }
    let mut builder = FieldSetBuilder::new();
    builder.length = Some(fieldset_length(spec));
    builder.date_fields = date_fields(spec);
    builder.time_precision = time_precision(spec);
    builder.year_style = spec.era.as_deref().map(|_| YearStyle::WithEra);
    builder.build_composite_datetime().map_err(err)
}

fn style_fieldset(spec: &DateTimeFormatSpec) -> Option<CompositeDateTimeFieldSet> {
    match (spec.date_style.as_deref(), spec.time_style.as_deref()) {
        (Some("short"), None) => Some(CompositeDateTimeFieldSet::Date(DateFieldSet::YMD(
            YMD::short(),
        ))),
        (Some("medium"), None) => Some(CompositeDateTimeFieldSet::Date(DateFieldSet::YMD(
            YMD::medium(),
        ))),
        (Some("long"), None) => Some(CompositeDateTimeFieldSet::Date(DateFieldSet::YMD(
            YMD::long(),
        ))),
        (Some("full"), None) => Some(CompositeDateTimeFieldSet::Date(DateFieldSet::YMDE(
            YMDE::long(),
        ))),
        (None, Some("short")) => Some(CompositeDateTimeFieldSet::Time(TimeFieldSet::T(
            T::short().with_time_precision(TimePrecision::Minute),
        ))),
        (None, Some("medium")) => Some(CompositeDateTimeFieldSet::Time(TimeFieldSet::T(
            T::medium(),
        ))),
        (None, Some("long") | Some("full")) => {
            Some(CompositeDateTimeFieldSet::Time(TimeFieldSet::T(T::long())))
        }
        (Some(date), Some(time)) => {
            let mut builder = FieldSetBuilder::new();
            builder.length = Some(match date {
                "full" | "long" => Length::Long,
                "short" => Length::Short,
                _ => Length::Medium,
            });
            builder.date_fields = Some(if date == "full" {
                DateFields::YMDE
            } else {
                DateFields::YMD
            });
            builder.time_precision = Some(if time == "short" {
                TimePrecision::Minute
            } else {
                TimePrecision::Second
            });
            builder.build_composite_datetime().ok()
        }
        _ => None,
    }
}

fn fieldset_length(spec: &DateTimeFormatSpec) -> Length {
    if let Some(style) = spec
        .month
        .as_deref()
        .or(spec.weekday.as_deref())
        .or(spec.era.as_deref())
        .filter(|style| matches!(*style, "long" | "short" | "narrow"))
    {
        return match style {
            "long" => Length::Long,
            "narrow" => Length::Short,
            _ => Length::Medium,
        };
    }
    match spec.date_style.as_deref().or(spec.time_style.as_deref()) {
        Some("full") | Some("long") => Length::Long,
        Some("short") | Some("2-digit") | Some("numeric") => Length::Short,
        _ if spec.month.as_deref() == Some("numeric")
            || spec.month.as_deref() == Some("2-digit")
            || spec.year.as_deref() == Some("numeric") =>
        {
            Length::Short
        }
        _ => Length::Medium,
    }
}

fn date_fields(spec: &DateTimeFormatSpec) -> Option<DateFields> {
    if spec.date_style.is_some() {
        return Some(DateFields::YMD);
    }
    let weekday = spec.weekday.is_some();
    let year = spec.year.is_some() || spec.era.is_some();
    let month = spec.month.is_some();
    let day = spec.day.is_some();
    match (year, month, day, weekday) {
        (true, true, true, true) => Some(DateFields::YMDE),
        (true, true, true, false) => Some(DateFields::YMD),
        (false, true, true, true) => Some(DateFields::MDE),
        (false, false, true, true) => Some(DateFields::DE),
        (false, true, true, false) => Some(DateFields::MD),
        (true, true, false, true) => Some(DateFields::YMDE),
        (true, true, false, false) => Some(DateFields::YM),
        (true, false, true, true) => Some(DateFields::YMDE),
        (true, false, true, false) => Some(DateFields::YMD),
        (true, false, false, true) => Some(DateFields::YMDE),
        (true, false, false, false) => Some(DateFields::Y),
        (false, true, false, true) => Some(DateFields::MDE),
        (false, true, false, false) => Some(DateFields::M),
        (false, false, true, false) => Some(DateFields::D),
        (false, false, false, true) => Some(DateFields::E),
        (false, false, false, false) => None,
    }
}

fn time_precision(spec: &DateTimeFormatSpec) -> Option<TimePrecision> {
    if let Some(digits) = spec.fractional_second_digits {
        let sub = match digits {
            1 => SubsecondDigits::S1,
            2 => SubsecondDigits::S2,
            _ => SubsecondDigits::S3,
        };
        return Some(TimePrecision::Subsecond(sub));
    }
    if spec.second.is_some() {
        return Some(TimePrecision::Second);
    }
    if spec.minute.is_some() {
        return Some(TimePrecision::Minute);
    }
    if spec.hour.is_some() || spec.day_period.is_some() {
        return Some(TimePrecision::Hour);
    }
    None
}

fn keep_fields(spec: &DateTimeFormatSpec) -> Option<Vec<&'static str>> {
    if spec.date_style.is_some() || spec.time_style.is_some() {
        return None;
    }
    let mut keep = Vec::new();
    if spec.weekday.is_some() {
        keep.push("weekday");
    }
    if spec.era.is_some() {
        keep.push("era");
    }
    if spec.year.is_some() {
        keep.push("year");
        keep.push("relatedYear");
        keep.push("yearName");
    }
    if spec.month.is_some() {
        keep.push("month");
    }
    if spec.day.is_some() {
        keep.push("day");
    }
    if spec.day_period.is_some() {
        keep.push("dayPeriod");
    }
    if spec.hour.is_some() {
        keep.push("hour");
        if matches!(spec.hour_cycle.as_deref(), Some("h11") | Some("h12")) {
            keep.push("dayPeriod");
        }
    }
    if spec.minute.is_some() {
        keep.push("minute");
    }
    if spec.second.is_some() || spec.fractional_second_digits.is_some() {
        keep.push("second");
        keep.push("fractionalSecond");
        keep.push("fraction");
        keep.push("decimal");
    }
    if spec.time_zone_name.is_some() {
        keep.push("timeZoneName");
    }
    Some(keep)
}

fn filter_parts(parts: Vec<FormatPart>, keep: &[&str]) -> Vec<FormatPart> {
    let mut out = Vec::new();
    let mut pending = Vec::new();
    let mut seen_field = false;
    for part in parts {
        if part.type_name == "literal" {
            if seen_field {
                pending.push(part);
            }
            continue;
        }
        if keep.contains(&part.type_name.as_str()) {
            if seen_field {
                out.append(&mut pending);
            }
            pending.clear();
            out.push(part);
            seen_field = true;
        } else {
            pending.clear();
        }
    }
    out
}

fn format_day_period(locale: &Locale, style: &str, time: Time) -> Result<String, String> {
    let mut names = FixedCalendarDateTimeNames::<Gregorian, TimeFieldSet>::try_new(locale.into())
        .map_err(err)?;
    let (length, pattern) = match style {
        "narrow" => (DayPeriodNameLength::Narrow, "BBBBB"),
        "short" => (DayPeriodNameLength::Abbreviated, "BBB"),
        _ => (DayPeriodNameLength::Wide, "BBBB"),
    };
    names.include_day_period_names(length).map_err(err)?;
    let pattern: DateTimePattern = pattern.parse().map_err(err)?;
    let rendered = names
        .with_pattern_unchecked(&pattern)
        .format(&time)
        .try_write_to_string()
        .map(|text| text.into_owned())
        .map_err(|_| "dayPeriod".to_owned())?;
    if rendered.chars().all(|ch| matches!(ch, 'B' | 'b')) {
        return Ok(flexible_day_period(time.hour.number(), style).into());
    }
    Ok(rendered)
}

fn flexible_day_period(hour: u8, style: &str) -> &'static str {
    let long = match hour {
        12 => "noon",
        0..=5 | 21..=23 => "at night",
        6..=11 => "in the morning",
        13..=17 => "in the afternoon",
        _ => "in the evening",
    };
    match style {
        "narrow" => match hour {
            12 => "n",
            0..=5 | 21..=23 => "at night",
            6..=11 => "in the morning",
            13..=17 => "in the afternoon",
            _ => "in the evening",
        },
        "short" => match hour {
            12 => "noon",
            0..=5 | 21..=23 => "at night",
            6..=11 => "in the morning",
            13..=17 => "in the afternoon",
            _ => "in the evening",
        },
        _ => long,
    }
}

fn time_of_day(millis_of_day: f64) -> Result<Time, String> {
    let hour = (millis_of_day / 3_600_000.0).floor() as u8;
    let minute = ((millis_of_day / 60_000.0).floor() as u32 % 60) as u8;
    let second = ((millis_of_day / 1_000.0).floor() as u32 % 60) as u8;
    let nano = ((millis_of_day % 1_000.0).round() as u32).saturating_mul(1_000_000);
    Time::try_new(hour, minute, second, nano).map_err(err)
}

/// TimeClip + 拆出 ISO 字段。越界返回错误，供 host 抛 RangeError。
pub fn components_from_millis(millis: f64) -> Result<(i32, u8, u8, u8, u8, u8, u32), String> {
    let (days, millis_of_day) = split_days(millis)?;
    let date = Date::from_rata_die(unix_epoch_rd() + days, Iso);
    let time = time_of_day(millis_of_day)?;
    Ok((
        date.year().extended_year(),
        date.month().ordinal,
        date.day_of_month().0,
        time.hour.number(),
        time.minute.number(),
        time.second.number(),
        u32::from(time.subsecond.number()),
    ))
}

fn split_days(millis: f64) -> Result<(i64, f64), String> {
    if !millis.is_finite() || millis.abs() > TIME_CLIP {
        return Err("Invalid time value".into());
    }
    let millis = millis.trunc();
    let days = (millis / MS_PER_DAY).floor();
    let mut millis_of_day = millis - days * MS_PER_DAY;
    if millis_of_day < 0.0 {
        millis_of_day += MS_PER_DAY;
    }
    Ok((days as i64, millis_of_day))
}

fn unix_epoch_rd() -> RataDie {
    Date::try_new_iso(1970, 1, 1)
        .expect("unix epoch")
        .to_rata_die()
}

fn remap_numeric(text: &str, numbering_system: &str) -> String {
    let remapped = crate::substitute_digits(text, numbering_system);
    if numbering_system == "arab" {
        remapped.replace('.', "\u{066b}")
    } else {
        remapped
    }
}

fn rewrite_year_digits(
    parts: &mut [FormatPart],
    year: i32,
    date_style: Option<&str>,
    year_style: Option<&str>,
) {
    let two_digit = date_style == Some("short") || year_style == Some("2-digit");
    for part in parts {
        if part.type_name != "year" && part.type_name != "relatedYear" {
            continue;
        }
        let digits: String = part
            .value
            .chars()
            .filter(|ch| ch.is_ascii_digit())
            .collect();
        if two_digit {
            if part.value.len() > 2 && digits.len() >= 2 {
                let suffix: String = digits
                    .chars()
                    .rev()
                    .take(2)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                part.value = suffix;
            }
            continue;
        }
        if year_style == Some("numeric") || date_style.is_none() {
            let expected = year.abs().to_string();
            if digits.len() < expected.len() {
                part.value = expected;
            }
        }
    }
}

fn pad_two_digit(part: &mut FormatPart, field: &str, style: Option<&str>, zero: char) {
    if part.type_name == field && style == Some("2-digit") && part.value.chars().count() == 1 {
        part.value.insert(0, zero);
    }
}

fn inject_cyclic_year_name(
    parts: &mut Vec<FormatPart>,
    rata_die: RataDie,
    calendar: &str,
    locale: &str,
) {
    if !matches!(calendar, "chinese" | "dangi") {
        return;
    }
    if parts.iter().any(|part| part.type_name == "yearName") {
        return;
    }
    let Some((_, name)) = cyclic_year(rata_die, calendar) else {
        return;
    };
    let year_name = FormatPart {
        type_name: "yearName".into(),
        value: name,
        source: None,
        unit: None,
    };
    if let Some(index) = parts
        .iter()
        .position(|part| part.type_name == "relatedYear" || part.type_name == "year")
    {
        parts.insert(index + 1, year_name);
        if locale_is_zh(locale) && !parts.iter().any(|part| part.value.contains('年')) {
            parts.insert(
                index + 2,
                FormatPart {
                    type_name: "literal".into(),
                    value: "年".into(),
                    source: None,
                    unit: None,
                },
            );
        }
    } else {
        parts.push(year_name);
        if locale_is_zh(locale) {
            parts.push(FormatPart {
                type_name: "literal".into(),
                value: "年".into(),
                source: None,
                unit: None,
            });
        }
    }
}

fn cyclic_year(rata_die: RataDie, calendar: &str) -> Option<(i32, String)> {
    let year = match calendar {
        "chinese" => Date::from_rata_die(rata_die, icu::calendar::cal::ChineseTraditional::new())
            .cyclic_year(),
        "dangi" => Date::from_rata_die(rata_die, icu::calendar::cal::KoreanTraditional::new())
            .cyclic_year(),
        _ => return None,
    };
    Some((year.related_iso, sexagenary_name(year.year)))
}

fn sexagenary_name(cycle_year: u8) -> String {
    const STEMS: [char; 10] = ['甲', '乙', '丙', '丁', '戊', '己', '庚', '辛', '壬', '癸'];
    const BRANCHES: [char; 12] = [
        '子', '丑', '寅', '卯', '辰', '巳', '午', '未', '申', '酉', '戌', '亥',
    ];
    let index = (cycle_year as usize).saturating_sub(1) % 60;
    format!("{}{}", STEMS[index % 10], BRANCHES[index % 12])
}

fn locale_is_zh(locale: &str) -> bool {
    locale.split(['-', '_']).next() == Some("zh")
}

fn numbering_zero(system: &str) -> char {
    match system {
        "arab" => '٠',
        "deva" => '०',
        "hanidec" => '〇',
        _ => '0',
    }
}

fn hour_cycle_pref(cycle: &str) -> Option<HourCycle> {
    match cycle {
        "h11" => Some(HourCycle::H11),
        "h12" => Some(HourCycle::H12),
        "h23" | "h24" => Some(HourCycle::H23),
        _ => None,
    }
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(date_style: &str) -> DateTimeFormatSpec {
        DateTimeFormatSpec {
            locale: "en-US".into(),
            calendar: "gregory".into(),
            numbering_system: "latn".into(),
            hour_cycle: None,
            date_style: Some(date_style.into()),
            time_style: None,
            weekday: None,
            era: None,
            year: None,
            month: None,
            day: None,
            day_period: None,
            hour: None,
            minute: None,
            second: None,
            fractional_second_digits: None,
            time_zone: "UTC".into(),
            time_zone_name: None,
        }
    }

    #[test]
    fn short_date_uses_two_digit_year() {
        let date = Date::try_new_iso(1886, 5, 1).expect("date");
        let days = date.to_rata_die() - unix_epoch_rd();
        let millis = days as f64 * MS_PER_DAY;
        let formatter = OwnedDateTimeFormatter::try_new(&spec("short")).expect("fmt");
        let text = formatter.format_millis(millis).expect("fmt");
        assert_eq!(text, "5/1/86", "millis={millis}");
    }
}
