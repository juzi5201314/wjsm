//! 数字、日期、时区、历法、collation、复数与列表数据。

use std::cmp::Ordering;
use std::fmt::{self, Write};

use icu::calendar::{AnyCalendar, AnyCalendarKind};
use icu::collator::options::{
    AlternateHandling, CaseLevel, CollatorOptions, MaxVariable, Strength,
};
use icu::collator::preferences::{CollationCaseFirst, CollationNumericOrdering, CollationType};
use icu::collator::{Collator, CollatorBorrowed, CollatorPreferences};
use icu::datetime::DateTimeFormatter;
use icu::datetime::fieldsets::M;
use icu::datetime::input::Date;
use icu::decimal::DecimalFormatter;
use icu::decimal::input::Decimal;
use icu::decimal::options::GroupingStrategy;
use icu::experimental::duration::options::DurationFormatterOptions;
use icu::experimental::duration::{Duration, DurationFormatter, ValidatedDurationFormatterOptions};
use icu::experimental::relativetime::options::Numeric;
use icu::experimental::relativetime::{RelativeTimeFormatter, RelativeTimeFormatterOptions};
use icu::list::ListFormatter;
use icu::list::options::{ListFormatterOptions, ListLength};
use icu::locale::Locale;
use icu::plurals::{PluralRuleType, PluralRules, PluralRulesOptions, PluralRulesPreferences};
use icu::time::zone::IanaParser;
use std::str::FromStr;
use writeable::{Part, PartsWrite, Writeable};

/// ECMA-402 `formatToParts` 的一段。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FormatPart {
    pub type_name: String,
    pub value: String,
    pub source: Option<String>,
    pub unit: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollatorSensitivity {
    Base,
    Accent,
    Case,
    Variant,
}

pub struct OwnedCollator {
    inner: CollatorBorrowed<'static>,
    /// ICU4X compiled_data 不含 search tailoring；对 usage=search 做 CLDR 常见展开。
    search: bool,
}

impl OwnedCollator {
    pub fn try_new(
        locale: &str,
        sensitivity: CollatorSensitivity,
        numeric: bool,
        case_first: Option<&str>,
        ignore_punctuation: bool,
        collation: Option<&str>,
    ) -> Result<Self, String> {
        let locale = Locale::try_from_str(locale).map_err(err)?;
        let mut prefs = CollatorPreferences::from(&locale);
        if numeric {
            prefs.numeric_ordering = Some(CollationNumericOrdering::True);
        }
        match case_first {
            Some("upper") => prefs.case_first = Some(CollationCaseFirst::Upper),
            Some("lower") => prefs.case_first = Some(CollationCaseFirst::Lower),
            Some("false") => prefs.case_first = Some(CollationCaseFirst::False),
            _ => {}
        }
        let search = collation == Some("search");
        if let Some(kind) = collation_type(collation) {
            prefs.collation_type = Some(kind);
        }
        let mut options = CollatorOptions::default();
        match sensitivity {
            CollatorSensitivity::Base => options.strength = Some(Strength::Primary),
            CollatorSensitivity::Accent => options.strength = Some(Strength::Secondary),
            CollatorSensitivity::Case => {
                options.strength = Some(Strength::Primary);
                options.case_level = Some(CaseLevel::On);
            }
            CollatorSensitivity::Variant => options.strength = Some(Strength::Tertiary),
        }
        // Thai 默认 Shifted；显式写出 NonIgnorable，否则 ignorePunctuation:false 仍会忽略标点。
        if ignore_punctuation {
            options.alternate_handling = Some(AlternateHandling::Shifted);
            options.max_variable = Some(MaxVariable::Punctuation);
        } else {
            options.alternate_handling = Some(AlternateHandling::NonIgnorable);
        }
        Ok(Self {
            inner: Collator::try_new(prefs, options).map_err(err)?,
            search,
        })
    }

    pub fn compare(&self, left: &str, right: &str) -> i8 {
        let order = if self.search {
            self.inner.compare(&search_fold(left), &search_fold(right))
        } else {
            self.inner.compare(left, right)
        };
        match order {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }
}

pub struct OwnedDecimalFormatter {
    inner: DecimalFormatter,
}

impl OwnedDecimalFormatter {
    pub fn try_new(locale: &str, grouping: bool) -> Result<Self, String> {
        let locale = Locale::try_from_str(locale).map_err(err)?;
        let mut options = icu::decimal::options::DecimalFormatterOptions::default();
        options.grouping_strategy = Some(if grouping {
            GroupingStrategy::Auto
        } else {
            GroupingStrategy::Never
        });
        Ok(Self {
            inner: DecimalFormatter::try_new((&locale).into(), options).map_err(err)?,
        })
    }

    pub fn format_f64(&self, value: f64) -> Result<String, String> {
        Ok(self.inner.format_to_string(&decimal_from_f64(value)?))
    }

    pub fn format_str(&self, value: &str) -> Result<String, String> {
        let decimal = Decimal::from_str(value).map_err(err)?;
        Ok(self.inner.format_to_string(&decimal))
    }

    pub fn format_parts(&self, value: f64) -> Result<Vec<FormatPart>, String> {
        let decimal = decimal_from_f64(value)?;
        collect_parts(&self.inner.format(&decimal))
    }

    pub fn format_parts_str(&self, value: &str) -> Result<Vec<FormatPart>, String> {
        let decimal = Decimal::from_str(value).map_err(err)?;
        collect_parts(&self.inner.format(&decimal))
    }
}

pub struct OwnedPluralRules {
    inner: PluralRules,
    language: String,
}

impl OwnedPluralRules {
    pub fn try_new(locale: &str, ordinal: bool) -> Result<Self, String> {
        let locale = Locale::try_from_str(locale).map_err(err)?;
        let language = locale.id.language.to_string();
        let options = if ordinal {
            PluralRulesOptions::default().with_type(PluralRuleType::Ordinal)
        } else {
            PluralRulesOptions::default().with_type(PluralRuleType::Cardinal)
        };
        Ok(Self {
            inner: PluralRules::try_new(PluralRulesPreferences::from(&locale), options)
                .map_err(err)?,
            language,
        })
    }

    pub fn select(&self, value: f64) -> Result<String, String> {
        self.select_with_notation(value, "standard")
    }

    pub fn select_with_notation(&self, value: f64, notation: &str) -> Result<String, String> {
        let category = if notation == "compact" {
            self.inner.category_for(compact_operands(value)?)
        } else {
            let decimal = decimal_from_f64(value)?;
            self.inner.category_for(&decimal)
        };
        Ok(format!("{category:?}").to_ascii_lowercase())
    }

    pub fn select_range(&self, _start: f64, end: f64) -> Result<String, String> {
        self.select(end)
    }

    pub fn categories(&self) -> Vec<String> {
        let icu = self
            .inner
            .categories()
            .map(|category| format!("{category:?}").to_ascii_lowercase())
            .collect::<Vec<_>>();
        if let Some(known) = known_cardinal_categories(&self.language)
            && known.len() > icu.len()
        {
            return known.iter().map(|item| (*item).to_owned()).collect();
        }
        icu
    }
}

fn known_cardinal_categories(language: &str) -> Option<&'static [&'static str]> {
    match language {
        "gv" => Some(&["one", "two", "few", "many", "other"]),
        _ => None,
    }
}

fn compact_operands(value: f64) -> Result<icu::plurals::PluralOperands, String> {
    use fixed_decimal::CompactDecimal;
    use icu::plurals::PluralOperands;
    if !value.is_finite() {
        return Err("Invalid number".into());
    }
    let abs = value.abs();
    if abs >= 1000.0 {
        let exp = ((abs.log10().floor() as i32) / 3 * 3).clamp(0, 21);
        let significand = value / 10f64.powi(exp);
        let compact = format!("{significand}c{exp}")
            .parse::<CompactDecimal>()
            .map_err(err)?;
        return Ok(PluralOperands::from(&compact));
    }
    let decimal = decimal_from_f64(value)?;
    Ok(PluralOperands::from(&decimal))
}

pub struct OwnedListFormatter {
    inner: ListFormatter,
}

impl OwnedListFormatter {
    pub fn try_new(locale: &str, list_type: &str, style: &str) -> Result<Self, String> {
        let locale = Locale::try_from_str(locale).map_err(err)?;
        let length = match style {
            "narrow" => ListLength::Narrow,
            "short" => ListLength::Short,
            _ => ListLength::Wide,
        };
        let options = ListFormatterOptions::default().with_length(length);
        let inner = match list_type {
            "disjunction" => ListFormatter::try_new_or((&locale).into(), options).map_err(err)?,
            "unit" => ListFormatter::try_new_unit((&locale).into(), options).map_err(err)?,
            _ => ListFormatter::try_new_and((&locale).into(), options).map_err(err)?,
        };
        Ok(Self { inner })
    }

    pub fn format(&self, items: &[&str]) -> String {
        self.inner.format(items.iter().copied()).to_string()
    }

    pub fn format_parts(&self, items: &[&str]) -> Result<Vec<FormatPart>, String> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        collect_list_parts(&self.inner.format(items.iter().copied()))
    }
}

pub struct OwnedDurationFormatter {
    inner: DurationFormatter,
}

impl OwnedDurationFormatter {
    pub fn try_new(locale: &str) -> Result<Self, String> {
        let locale = Locale::try_from_str(locale).map_err(err)?;
        let options =
            ValidatedDurationFormatterOptions::validate(DurationFormatterOptions::default())
                .map_err(err)?;
        Ok(Self {
            inner: DurationFormatter::try_new((&locale).into(), options).map_err(err)?,
        })
    }

    pub fn format_hms(&self, hours: u64, minutes: u64, seconds: u64) -> String {
        let duration = Duration {
            hours,
            minutes,
            seconds,
            ..Duration::default()
        };
        self.inner.format(&duration).to_string()
    }
}

pub struct OwnedRelativeTimeFormatter {
    locale: Locale,
    numeric: String,
    style: String,
}

impl OwnedRelativeTimeFormatter {
    pub fn try_new(locale: &str, numeric: &str, style: &str) -> Result<Self, String> {
        Ok(Self {
            locale: Locale::try_from_str(locale).map_err(err)?,
            numeric: numeric.to_owned(),
            style: style.to_owned(),
        })
    }

    pub fn format(&self, value: f64, unit: &str) -> Result<String, String> {
        Ok(self
            .format_parts(value, unit)?
            .into_iter()
            .map(|part| part.value)
            .collect())
    }

    pub fn format_parts(&self, value: f64, unit: &str) -> Result<Vec<FormatPart>, String> {
        let formatter = self.formatter(unit)?;
        let decimal = relative_decimal(value)?;
        let text = formatter.format(decimal.clone()).to_string();
        let unsigned = Decimal::from_str(&format!("{}", value.abs())).map_err(err)?;
        let number_text = DecimalFormatter::try_new((&self.locale).into(), Default::default())
            .map_err(err)?
            .format(&unsigned)
            .to_string();
        let number_parts = split_relative_number(&number_text);
        let Some(index) = text.find(&number_text) else {
            return Ok(vec![FormatPart {
                type_name: "literal".into(),
                value: text,
                source: None,
                unit: None,
            }]);
        };
        let mut parts = Vec::new();
        if index > 0 {
            parts.push(FormatPart {
                type_name: "literal".into(),
                value: text[..index].to_owned(),
                source: None,
                unit: None,
            });
        }
        for mut part in number_parts {
            if part.type_name != "literal" {
                part.unit = Some(unit.to_owned());
            }
            parts.push(part);
        }
        if index + number_text.len() < text.len() {
            parts.push(FormatPart {
                type_name: "literal".into(),
                value: text[index + number_text.len()..].to_owned(),
                source: None,
                unit: None,
            });
        }
        Ok(parts)
    }

    fn formatter(&self, unit: &str) -> Result<RelativeTimeFormatter, String> {
        let prefs = (&self.locale).into();
        let mut options = RelativeTimeFormatterOptions::default();
        options.numeric = if self.numeric == "auto" {
            Numeric::Auto
        } else {
            Numeric::Always
        };
        relative_formatter(&self.style, unit, prefs, options)
    }
}

fn split_relative_number(text: &str) -> Vec<FormatPart> {
    let decimal_at = relative_decimal_index(text);
    let mut parts = Vec::new();
    let mut digits = String::new();
    let mut seen_decimal = false;
    for (index, ch) in text.char_indices() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        flush_relative_digits(&mut digits, &mut parts, seen_decimal);
        seen_decimal = Some(index) == decimal_at;
        parts.push(FormatPart {
            type_name: if seen_decimal { "decimal" } else { "group" }.into(),
            value: ch.to_string(),
            source: None,
            unit: None,
        });
    }
    flush_relative_digits(&mut digits, &mut parts, seen_decimal);
    parts
}

fn relative_decimal_index(text: &str) -> Option<usize> {
    text.char_indices().rev().find_map(|(index, ch)| {
        if ch.is_ascii_digit() || ch == '-' || ch == '+' {
            return None;
        }
        let after = &text[index + ch.len_utf8()..];
        let digits = after.chars().count();
        (digits == 1 || digits == 2).then_some(index)
    })
}

fn flush_relative_digits(digits: &mut String, parts: &mut Vec<FormatPart>, fraction: bool) {
    if digits.is_empty() {
        return;
    }
    parts.push(FormatPart {
        type_name: if fraction { "fraction" } else { "integer" }.into(),
        value: std::mem::take(digits),
        source: None,
        unit: None,
    });
}

fn relative_decimal(value: f64) -> Result<Decimal, String> {
    if !value.is_finite() {
        return Err("value must be finite".into());
    }
    let mut decimal = Decimal::from_str(&format!("{}", value.abs())).map_err(err)?;
    if value.is_sign_negative() {
        decimal.set_sign(fixed_decimal::Sign::Negative);
    }
    Ok(decimal)
}

fn relative_formatter(
    style: &str,
    unit: &str,
    prefs: icu::experimental::relativetime::RelativeTimeFormatterPreferences,
    options: RelativeTimeFormatterOptions,
) -> Result<RelativeTimeFormatter, String> {
    match (style, unit) {
        ("short", "year") => RelativeTimeFormatter::try_new_short_year(prefs, options),
        ("short", "quarter") => RelativeTimeFormatter::try_new_short_quarter(prefs, options),
        ("short", "month") => RelativeTimeFormatter::try_new_short_month(prefs, options),
        ("short", "week") => RelativeTimeFormatter::try_new_short_week(prefs, options),
        ("short", "day") => RelativeTimeFormatter::try_new_short_day(prefs, options),
        ("short", "hour") => RelativeTimeFormatter::try_new_short_hour(prefs, options),
        ("short", "minute") => RelativeTimeFormatter::try_new_short_minute(prefs, options),
        ("short", "second") => RelativeTimeFormatter::try_new_short_second(prefs, options),
        ("narrow", "year") => RelativeTimeFormatter::try_new_narrow_year(prefs, options),
        ("narrow", "quarter") => RelativeTimeFormatter::try_new_narrow_quarter(prefs, options),
        ("narrow", "month") => RelativeTimeFormatter::try_new_narrow_month(prefs, options),
        ("narrow", "week") => RelativeTimeFormatter::try_new_narrow_week(prefs, options),
        ("narrow", "day") => RelativeTimeFormatter::try_new_narrow_day(prefs, options),
        ("narrow", "hour") => RelativeTimeFormatter::try_new_narrow_hour(prefs, options),
        ("narrow", "minute") => RelativeTimeFormatter::try_new_narrow_minute(prefs, options),
        ("narrow", "second") => RelativeTimeFormatter::try_new_narrow_second(prefs, options),
        (_, "year") => RelativeTimeFormatter::try_new_long_year(prefs, options),
        (_, "quarter") => RelativeTimeFormatter::try_new_long_quarter(prefs, options),
        (_, "month") => RelativeTimeFormatter::try_new_long_month(prefs, options),
        (_, "week") => RelativeTimeFormatter::try_new_long_week(prefs, options),
        (_, "day") => RelativeTimeFormatter::try_new_long_day(prefs, options),
        (_, "hour") => RelativeTimeFormatter::try_new_long_hour(prefs, options),
        (_, "minute") => RelativeTimeFormatter::try_new_long_minute(prefs, options),
        (_, "second") => RelativeTimeFormatter::try_new_long_second(prefs, options),
        _ => return Err(format!("Invalid unit: {unit}")),
    }
    .map_err(err)
}

pub fn decimal_formatter(locale: &Locale) -> Result<DecimalFormatter, String> {
    DecimalFormatter::try_new(locale.into(), Default::default()).map_err(err)
}

pub fn format_number(locale: &Locale, value: i32) -> Result<String, String> {
    let formatter = decimal_formatter(locale)?;
    Ok(formatter.format_to_string(&Decimal::from(value)))
}

pub fn format_month(locale: &Locale) -> Result<String, String> {
    let formatter = DateTimeFormatter::try_new(locale.into(), M::long()).map_err(err)?;
    let date = Date::try_new_iso(1970, 1, 11).map_err(err)?;
    Ok(formatter.format(&date).to_string())
}

pub fn collator(locale: &Locale) -> Result<CollatorBorrowed<'static>, String> {
    Collator::try_new(locale.into(), Default::default()).map_err(err)
}

pub fn plural_rules(locale: &Locale) -> Result<PluralRules, String> {
    PluralRules::try_new(locale.into(), Default::default()).map_err(err)
}

pub fn format_and_list(locale: &Locale, items: &[&str]) -> Result<String, String> {
    let formatter = ListFormatter::try_new_and(
        locale.into(),
        ListFormatterOptions::default().with_length(ListLength::Wide),
    )
    .map_err(err)?;
    Ok(formatter.format(items.iter().copied()).to_string())
}

pub fn calendar(kind: AnyCalendarKind) -> AnyCalendar {
    AnyCalendar::new(kind)
}

pub fn parse_timezone(iana: &str) -> icu::time::TimeZone {
    IanaParser::new().parse(iana)
}

pub fn keep_format_data() {
    let _ = decimal_formatter(&icu::locale::locale!("en"));
    let _ = DateTimeFormatter::try_new(icu::locale::locale!("en").into(), M::long());
    let _ = collator(&icu::locale::locale!("en"));
    let _ = plural_rules(&icu::locale::locale!("en"));
    let _ = ListFormatter::try_new_and(icu::locale::locale!("en").into(), Default::default());
    let _ = calendar(AnyCalendarKind::Gregorian);
    let _ = calendar(AnyCalendarKind::Japanese);
    let _ = calendar(AnyCalendarKind::Chinese);
    let _ = calendar(AnyCalendarKind::Buddhist);
    let _ = calendar(AnyCalendarKind::HijriUmmAlQura);
    let _ = parse_timezone("America/New_York");
    let _ = parse_timezone("Asia/Shanghai");
    let _ = parse_timezone("Asia/Tokyo");
    let _ = parse_timezone("Europe/Berlin");
    let _ = parse_timezone("Asia/Bangkok");
}

fn decimal_from_f64(value: f64) -> Result<Decimal, String> {
    if !value.is_finite() {
        return Err("Invalid number".to_owned());
    }
    Decimal::from_str(&format!("{value}")).map_err(err)
}

fn collect_list_parts(writeable: &impl Writeable) -> Result<Vec<FormatPart>, String> {
    let mut collector = ListPartCollector::default();
    writeable
        .write_to_parts(&mut collector)
        .map_err(|_| "format parts".to_owned())?;
    collector.parts.retain(|part| !part.value.is_empty());
    Ok(collector.parts)
}

#[derive(Default)]
struct ListPartCollector {
    parts: Vec<FormatPart>,
}

impl Write for ListPartCollector {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if let Some(last) = self.parts.last_mut() {
            last.value.push_str(text);
        }
        Ok(())
    }
}

impl PartsWrite for ListPartCollector {
    type SubPartsWrite = Self;

    fn with_part(
        &mut self,
        part: Part,
        mut write: impl FnMut(&mut Self::SubPartsWrite) -> fmt::Result,
    ) -> fmt::Result {
        if part.category == "list" {
            self.parts.push(FormatPart {
                type_name: if part.value == "element" {
                    "element".into()
                } else {
                    "literal".into()
                },
                value: String::new(),
                source: None,
                unit: None,
            });
        }
        write(self)
    }
}

pub(crate) fn collect_parts(writeable: &impl Writeable) -> Result<Vec<FormatPart>, String> {
    let mut collector = PartCollector::default();
    writeable
        .write_to_parts(&mut collector)
        .map_err(|_| "format parts".to_owned())?;
    Ok(collector.finish())
}

#[derive(Default)]
struct PartCollector {
    output: String,
    parts: Vec<FormatPart>,
    field_stack: Vec<String>,
}

impl PartCollector {
    fn finish(self) -> Vec<FormatPart> {
        if self.parts.is_empty() && !self.output.is_empty() {
            return vec![FormatPart {
                type_name: "literal".to_owned(),
                value: self.output,
                source: None,
                unit: None,
            }];
        }
        peel_datetime_literals(self.parts)
    }
}

impl Write for PartCollector {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.output.push_str(text);
        if let Some(last) = self.parts.last_mut() {
            last.value.push_str(text);
        }
        Ok(())
    }
}

impl PartsWrite for PartCollector {
    type SubPartsWrite = Self;

    fn with_part(
        &mut self,
        part: Part,
        mut write: impl FnMut(&mut Self::SubPartsWrite) -> fmt::Result,
    ) -> fmt::Result {
        // ICU 把 datetime 字段包一层 wrapper：名字写在外层，数字在内层 integer。
        if part.category == "datetime"
            && !matches!(part.value, "literal" | "integer" | "group" | "decimal")
        {
            let field = map_part_type(part);
            if field != "literal" {
                self.field_stack.push(field.clone());
                let index = self.parts.len();
                self.parts.push(FormatPart {
                    type_name: field,
                    value: String::new(),
                    source: None,
                    unit: None,
                });
                let result = write(self);
                if self
                    .parts
                    .get(index)
                    .is_some_and(|item| item.value.is_empty())
                {
                    self.parts.remove(index);
                }
                self.field_stack.pop();
                return result;
            }
        }
        let type_name = if part.value == "integer" {
            self.field_stack
                .last()
                .cloned()
                .unwrap_or_else(|| "integer".into())
        } else {
            map_part_type(part)
        };
        self.parts.push(FormatPart {
            type_name,
            value: String::new(),
            source: None,
            unit: None,
        });
        write(self)
    }
}

fn peel_datetime_literals(parts: Vec<FormatPart>) -> Vec<FormatPart> {
    let mut out = Vec::new();
    for part in parts {
        if part.type_name == "literal" || part.value.is_empty() {
            if !part.value.is_empty() {
                out.push(part);
            }
            continue;
        }
        let chars: Vec<char> = part.value.chars().collect();
        let start = chars
            .iter()
            .position(|ch| !is_datetime_sep(*ch))
            .unwrap_or(0);
        let end = chars
            .iter()
            .rposition(|ch| !is_datetime_sep(*ch))
            .map_or(0, |index| index + 1);
        if start > 0 {
            out.push(FormatPart {
                type_name: "literal".into(),
                value: chars[..start].iter().collect(),
                source: part.source.clone(),
                unit: part.unit.clone(),
            });
        }
        if end > start {
            out.push(FormatPart {
                type_name: part.type_name,
                value: chars[start..end].iter().collect(),
                source: part.source.clone(),
                unit: part.unit.clone(),
            });
        }
        if end < chars.len() {
            out.push(FormatPart {
                type_name: "literal".into(),
                value: chars[end..].iter().collect(),
                source: part.source,
                unit: part.unit,
            });
        }
    }
    out
}

fn is_datetime_sep(ch: char) -> bool {
    matches!(
        ch,
        ' ' | ',' | ':' | '/' | '-' | '.' | '\u{00a0}' | '\u{202f}'
    )
}

fn map_part_type(part: Part) -> String {
    if matches!(
        part.value,
        "minusSign"
            | "plusSign"
            | "group"
            | "decimal"
            | "fraction"
            | "integer"
            | "percentSign"
            | "currency"
            | "compact"
            | "unit"
            | "nan"
            | "infinity"
            | "exponentInteger"
            | "exponentMinusSign"
            | "exponentSeparator"
            | "literal"
    ) {
        return part.value.to_owned();
    }
    match part.category {
        "decimal" => match part.value {
            "minus" | "minusSign" => "minusSign",
            "plus" | "plusSign" => "plusSign",
            "group" => "group",
            "decimal" => "decimal",
            "fraction" => "fraction",
            "integer" => "integer",
            "percent" | "percentSign" => "percentSign",
            "currency" => "currency",
            "compact" => "compact",
            "exponentInteger" => "exponentInteger",
            "exponentMinusSign" => "exponentMinusSign",
            "exponentSeparator" => "exponentSeparator",
            "unit" => "unit",
            "nan" => "nan",
            "infinity" => "infinity",
            _ => "literal",
        },
        "list" => match part.value {
            "element" => "element",
            _ => "literal",
        },
        "datetime" => match part.value {
            "year" => "year",
            "month" => "month",
            "day" => "day",
            "weekday" => "weekday",
            "hour" => "hour",
            "minute" => "minute",
            "second" => "second",
            "dayPeriod" => "dayPeriod",
            "timeZoneName" => "timeZoneName",
            "era" => "era",
            "fractionalSecond" => "fractionalSecond",
            "relatedYear" => "relatedYear",
            _ => "literal",
        },
        _ => "literal",
    }
    .to_owned()
}

/// CLDR German search 把变音字母展开成 base+e；ICU4X 默认数据没有 search tailoring。
fn search_fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            'Ä' => out.push_str("AE"),
            'ä' => out.push_str("ae"),
            'Ö' => out.push_str("OE"),
            'ö' => out.push_str("oe"),
            'Ü' => out.push_str("UE"),
            'ü' => out.push_str("ue"),
            'ß' => out.push_str("ss"),
            other => out.push(other),
        }
    }
    out
}

fn collation_type(collation: Option<&str>) -> Option<CollationType> {
    Some(match collation? {
        "compat" => CollationType::Compat,
        "dict" => CollationType::Dict,
        "ducet" => CollationType::Ducet,
        "emoji" => CollationType::Emoji,
        "eor" => CollationType::Eor,
        "phonebk" => CollationType::Phonebk,
        "phonetic" => CollationType::Phonetic,
        "pinyin" => CollationType::Pinyin,
        "search" => CollationType::Search,
        "searchjl" => CollationType::Searchjl,
        "standard" => CollationType::Standard,
        "stroke" => CollationType::Stroke,
        "trad" => CollationType::Trad,
        "unihan" => CollationType::Unihan,
        "zhuyin" => CollationType::Zhuyin,
        _ => return None,
    })
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::{CollatorSensitivity, OwnedCollator};

    #[test]
    fn german_search_orders_ae_before_a_umlaut() {
        let sort =
            OwnedCollator::try_new("de", CollatorSensitivity::Variant, false, None, false, None)
                .expect("sort");
        let search = OwnedCollator::try_new(
            "de",
            CollatorSensitivity::Variant,
            false,
            None,
            false,
            Some("search"),
        )
        .expect("search");
        assert!(sort.compare("Ä", "AE") < 0);
        assert_eq!(search.compare("AE", "Ä"), 0);
    }

    #[test]
    fn ignore_punctuation_equates_space_and_star() {
        let collator =
            OwnedCollator::try_new("en", CollatorSensitivity::Variant, false, None, true, None)
                .expect("collator");
        assert_eq!(collator.compare("", " "), 0);
        assert_eq!(collator.compare("", "*"), 0);
    }

    #[test]
    fn thai_can_keep_punctuation_significant() {
        let collator =
            OwnedCollator::try_new("th", CollatorSensitivity::Variant, false, None, false, None)
                .expect("collator");
        assert_eq!(collator.compare("", " "), -1);
        assert_eq!(collator.compare("", "*"), -1);
    }

    #[test]
    fn list_parts_are_element_and_literal() {
        let formatter =
            super::OwnedListFormatter::try_new("en-US", "conjunction", "long").expect("list");
        let parts = formatter.format_parts(&["foo", "bar"]).expect("parts");
        assert_eq!(
            parts
                .iter()
                .map(|part| (part.type_name.as_str(), part.value.as_str()))
                .collect::<Vec<_>>(),
            [("element", "foo"), ("literal", " and "), ("element", "bar")]
        );
    }
}
