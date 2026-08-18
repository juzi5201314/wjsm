//! Intl 实例内部槽。不泄漏到 JS 自有属性。

use wjsm_intl_data::{
    OwnedCollator, OwnedDateTimeFormatter, OwnedDisplayNames, OwnedListFormatter, OwnedPluralRules,
    OwnedRelativeTimeFormatter, OwnedSegmenter,
};

#[allow(clippy::large_enum_variant)]
pub(crate) enum IntlSlot {
    Locale(LocaleSlot),
    Collator(CollatorSlot),
    NumberFormat(NumberFormatSlot),
    DateTimeFormat(DateTimeFormatSlot),
    PluralRules(PluralRulesSlot),
    ListFormat(ListFormatSlot),
    RelativeTime(RelativeTimeSlot),
    DisplayNames(DisplayNamesSlot),
    Segmenter(SegmenterSlot),
    Segments(SegmentsSlot),
    SegmentIterator(SegmentIterSlot),
    DurationFormat(DurationFormatSlot),
}

pub(crate) struct LocaleSlot {
    pub tag: String,
    pub language: String,
    pub script: Option<String>,
    pub region: Option<String>,
    pub variants: Vec<String>,
    pub calendar: Option<String>,
    pub collation: Option<String>,
    pub hour_cycle: Option<String>,
    pub case_first: Option<String>,
    pub numeric: bool,
    pub numbering_system: Option<String>,
    pub first_day_of_week: Option<String>,
}

pub(crate) struct CollatorSlot {
    pub locale: String,
    pub usage: String,
    pub sensitivity: String,
    pub ignore_punctuation: bool,
    pub numeric: bool,
    pub case_first: String,
    pub collation: String,
    pub bound_compare: Option<i64>,
    pub formatter: Option<OwnedCollator>,
}

pub(crate) struct NumberFormatSlot {
    pub locale: String,
    pub numbering_system: String,
    pub style: String,
    pub currency: Option<String>,
    pub currency_display: String,
    pub currency_sign: String,
    pub unit: Option<String>,
    pub unit_display: String,
    pub notation: String,
    pub compact_display: String,
    pub sign_display: String,
    pub use_grouping: String,
    pub minimum_integer_digits: u32,
    pub minimum_fraction_digits: u32,
    pub maximum_fraction_digits: u32,
    pub minimum_significant_digits: Option<u32>,
    pub maximum_significant_digits: Option<u32>,
    pub rounding_mode: String,
    pub rounding_increment: u32,
    pub rounding_priority: String,
    pub trailing_zero_display: String,
    pub bound_format: Option<i64>,
    pub formatter: Option<wjsm_intl_data::OwnedNumberFormatter>,
}

pub(crate) struct DateTimeFormatSlot {
    pub locale: String,
    pub calendar: String,
    pub numbering_system: String,
    pub time_zone: String,
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
    pub time_zone_name: Option<String>,
    pub bound_format: Option<i64>,
    pub formatter: Option<OwnedDateTimeFormatter>,
}

pub(crate) struct PluralRulesSlot {
    pub locale: String,
    pub type_name: String,
    pub notation: String,
    pub minimum_integer_digits: u32,
    pub minimum_fraction_digits: u32,
    pub maximum_fraction_digits: u32,
    pub minimum_significant_digits: Option<u32>,
    pub maximum_significant_digits: Option<u32>,
    pub rounding_increment: u32,
    pub rounding_mode: String,
    pub rounding_priority: String,
    pub trailing_zero_display: String,
    pub formatter: Option<OwnedPluralRules>,
}

pub(crate) struct ListFormatSlot {
    pub locale: String,
    pub type_name: String,
    pub style: String,
    pub formatter: Option<OwnedListFormatter>,
}

pub(crate) struct RelativeTimeSlot {
    pub locale: String,
    pub numbering_system: String,
    pub numeric: String,
    pub style: String,
    pub formatter: Option<OwnedRelativeTimeFormatter>,
}

pub(crate) struct DisplayNamesSlot {
    pub locale: String,
    pub style: String,
    pub type_name: String,
    pub fallback: String,
    pub language_display: String,
    pub formatter: Option<OwnedDisplayNames>,
}

pub(crate) struct SegmenterSlot {
    pub locale: String,
    pub granularity: String,
    pub formatter: OwnedSegmenter,
}

pub(crate) struct SegmentsSlot {
    pub text: String,
    pub granularity: String,
    pub breaks: Vec<u32>,
    pub word_likes: Vec<bool>,
}

pub(crate) struct SegmentIterSlot {
    pub text: String,
    pub granularity: String,
    pub breaks: Vec<u32>,
    pub word_likes: Vec<bool>,
    pub index: usize,
}

#[derive(Clone)]
pub(crate) struct DurationUnitSlot {
    pub style: String,
    pub display: String,
}

pub(crate) struct DurationFormatSlot {
    pub locale: String,
    pub numbering_system: String,
    pub style: String,
    pub units: Vec<(String, DurationUnitSlot)>,
    pub fractional_digits: Option<u32>,
}

impl IntlSlot {
    pub(crate) fn bound_roots(&self) -> impl Iterator<Item = i64> {
        let bound = match self {
            Self::Collator(slot) => slot.bound_compare,
            Self::NumberFormat(slot) => slot.bound_format,
            Self::DateTimeFormat(slot) => slot.bound_format,
            _ => None,
        };
        bound.into_iter()
    }
}
