//! 各构造器的已校验选项记录。host 负责从 JS 取值并填这些结构。

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocaleRecord {
    pub language: Option<String>,
    pub script: Option<String>,
    pub region: Option<String>,
    pub calendar: Option<String>,
    pub collation: Option<String>,
    pub hour_cycle: Option<String>,
    pub case_first: Option<String>,
    pub numbering_system: Option<String>,
    pub numeric: Option<bool>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CollatorRecord {
    pub locale_matcher: String,
    pub usage: String,
    pub sensitivity: Option<String>,
    pub ignore_punctuation: bool,
    pub numeric: bool,
    pub case_first: Option<String>,
    pub collation: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NumberFormatRecord {
    pub locale_matcher: String,
    pub style: String,
    pub currency: Option<String>,
    pub currency_display: String,
    pub currency_sign: String,
    pub unit: Option<String>,
    pub unit_display: String,
    pub notation: String,
    pub compact_display: String,
    pub numbering_system: Option<String>,
    pub sign_display: String,
    pub use_grouping: Option<String>,
    pub minimum_integer_digits: u32,
    pub minimum_fraction_digits: Option<u32>,
    pub maximum_fraction_digits: Option<u32>,
    pub minimum_significant_digits: Option<u32>,
    pub maximum_significant_digits: Option<u32>,
    pub rounding_mode: String,
    pub rounding_increment: u32,
    pub trailing_zero_display: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DateTimeFormatRecord {
    pub locale_matcher: String,
    pub calendar: Option<String>,
    pub numbering_system: Option<String>,
    pub hour_cycle: Option<String>,
    pub time_zone: Option<String>,
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
    pub time_zone_name: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PluralRulesRecord {
    pub locale_matcher: String,
    pub r#type: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListFormatRecord {
    pub locale_matcher: String,
    pub r#type: String,
    pub style: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelativeTimeRecord {
    pub locale_matcher: String,
    pub numeric: String,
    pub style: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DisplayNamesRecord {
    pub locale_matcher: String,
    pub style: String,
    pub r#type: String,
    pub fallback: String,
    pub language_display: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SegmenterRecord {
    pub locale_matcher: String,
    pub granularity: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DurationFormatRecord {
    pub locale_matcher: String,
    pub style: String,
}
