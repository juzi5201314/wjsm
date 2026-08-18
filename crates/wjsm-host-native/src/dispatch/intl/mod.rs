//! ECMA-402 `Intl` 命名空间、构造器与内部槽。

mod collator;
mod common;
mod constructors;
mod datetime_format;
mod display_names;
mod duration_format;
mod install;
mod js;
mod list_format;
mod locale;
mod locale_methods;
mod number_format;
mod number_format_options;
mod plural_rules;
mod relative_time;
mod segmenter;
mod slots;

use std::collections::HashMap;

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, type_error};
use crate::NativeAgentState;

pub(crate) use install::{ensure_constructor_prototype, ensure_intl_object};
pub(crate) use locale_methods::{
    DateLocaleKind, date_to_locale, ensure_bigint_prototype, ensure_number_prototype,
    ensure_string_prototype, install_array_to_locale_string, primitive_locale_property,
};
pub(crate) use slots::IntlSlot;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum IntlCallable {
    GetCanonicalLocales,
    SupportedValuesOf,
    LocaleConstructor,
    LocaleMaximize,
    LocaleMinimize,
    LocaleToString,
    LocaleGetCalendars,
    LocaleGetCollations,
    LocaleGetHourCycles,
    LocaleGetNumberingSystems,
    LocaleGetTimeZones,
    LocaleGetTextInfo,
    LocaleGetWeekInfo,
    LocaleLanguage,
    LocaleScript,
    LocaleRegion,
    LocaleBaseName,
    LocaleCalendar,
    LocaleCollation,
    LocaleHourCycle,
    LocaleCaseFirst,
    LocaleNumeric,
    LocaleNumberingSystem,
    LocaleFirstDayOfWeek,
    LocaleVariants,
    CollatorConstructor,
    CollatorSupportedLocalesOf,
    CollatorResolvedOptions,
    CollatorCompareGet,
    CollatorCompare(u32),
    NumberFormatConstructor,
    NumberFormatSupportedLocalesOf,
    NumberFormatResolvedOptions,
    NumberFormatFormatGet,
    NumberFormatFormat(u32),
    NumberFormatFormatToParts,
    NumberFormatFormatRange,
    NumberFormatFormatRangeToParts,
    DateTimeFormatConstructor,
    DateTimeFormatSupportedLocalesOf,
    DateTimeFormatResolvedOptions,
    DateTimeFormatFormatGet,
    DateTimeFormatFormat(u32),
    DateTimeFormatFormatToParts,
    DateTimeFormatFormatRange,
    DateTimeFormatFormatRangeToParts,
    PluralRulesConstructor,
    PluralRulesSupportedLocalesOf,
    PluralRulesResolvedOptions,
    PluralRulesSelect,
    PluralRulesSelectRange,
    ListFormatConstructor,
    ListFormatSupportedLocalesOf,
    ListFormatResolvedOptions,
    ListFormatFormat,
    ListFormatFormatToParts,
    RelativeTimeFormatConstructor,
    RelativeTimeFormatSupportedLocalesOf,
    RelativeTimeFormatResolvedOptions,
    RelativeTimeFormatFormat,
    RelativeTimeFormatFormatToParts,
    DisplayNamesConstructor,
    DisplayNamesSupportedLocalesOf,
    DisplayNamesResolvedOptions,
    DisplayNamesOf,
    SegmenterConstructor,
    SegmenterSupportedLocalesOf,
    SegmenterResolvedOptions,
    SegmenterSegment,
    SegmentsContaining,
    SegmentsIterator,
    SegmentIteratorNext,
    DurationFormatConstructor,
    DurationFormatSupportedLocalesOf,
    DurationFormatResolvedOptions,
    DurationFormatFormat,
    DurationFormatFormatToParts,
    StringNormalize,
    StringToLowerCase,
    StringToUpperCase,
    StringToLocaleLowerCase,
    StringToLocaleUpperCase,
    StringLocaleCompare,
    NumberToLocaleString,
    BigIntToLocaleString,
    ArrayToLocaleString,
}

#[derive(Default)]
pub(crate) struct IntlState {
    pub object: Option<i64>,
    pub slots: HashMap<u32, IntlSlot>,
    pub locale_prototype: Option<i64>,
    pub collator_prototype: Option<i64>,
    pub number_format_prototype: Option<i64>,
    pub datetime_format_prototype: Option<i64>,
    pub plural_rules_prototype: Option<i64>,
    pub list_format_prototype: Option<i64>,
    pub relative_time_prototype: Option<i64>,
    pub display_names_prototype: Option<i64>,
    pub segmenter_prototype: Option<i64>,
    pub segments_prototype: Option<i64>,
    pub segment_iterator_prototype: Option<i64>,
    pub duration_format_prototype: Option<i64>,
    pub string_prototype: Option<i64>,
    pub number_prototype: Option<i64>,
    pub bigint_prototype: Option<i64>,
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: IntlCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    constructors::call(ctx, state, callable, receiver, args)
}

pub(crate) fn metadata(kind: IntlCallable) -> Option<(&'static str, u32)> {
    Some(match kind {
        IntlCallable::GetCanonicalLocales => ("getCanonicalLocales", 1),
        IntlCallable::SupportedValuesOf => ("supportedValuesOf", 1),
        IntlCallable::LocaleConstructor => ("Locale", 1),
        IntlCallable::LocaleMaximize => ("maximize", 0),
        IntlCallable::LocaleMinimize => ("minimize", 0),
        IntlCallable::LocaleToString => ("toString", 0),
        IntlCallable::LocaleGetCalendars => ("getCalendars", 0),
        IntlCallable::LocaleGetCollations => ("getCollations", 0),
        IntlCallable::LocaleGetHourCycles => ("getHourCycles", 0),
        IntlCallable::LocaleGetNumberingSystems => ("getNumberingSystems", 0),
        IntlCallable::LocaleGetTimeZones => ("getTimeZones", 0),
        IntlCallable::LocaleGetTextInfo => ("getTextInfo", 0),
        IntlCallable::LocaleGetWeekInfo => ("getWeekInfo", 0),
        IntlCallable::CollatorConstructor => ("Collator", 0),
        IntlCallable::CollatorSupportedLocalesOf => ("supportedLocalesOf", 1),
        IntlCallable::CollatorResolvedOptions => ("resolvedOptions", 0),
        IntlCallable::CollatorCompareGet => ("get compare", 0),
        IntlCallable::CollatorCompare(_) => ("", 2),
        IntlCallable::NumberFormatConstructor => ("NumberFormat", 0),
        IntlCallable::NumberFormatSupportedLocalesOf => ("supportedLocalesOf", 1),
        IntlCallable::NumberFormatResolvedOptions => ("resolvedOptions", 0),
        IntlCallable::NumberFormatFormatGet => ("get format", 0),
        IntlCallable::NumberFormatFormat(_) => ("", 1),
        IntlCallable::NumberFormatFormatToParts => ("formatToParts", 1),
        IntlCallable::NumberFormatFormatRange => ("formatRange", 2),
        IntlCallable::NumberFormatFormatRangeToParts => ("formatRangeToParts", 2),
        IntlCallable::DateTimeFormatConstructor => ("DateTimeFormat", 0),
        IntlCallable::DateTimeFormatSupportedLocalesOf => ("supportedLocalesOf", 1),
        IntlCallable::DateTimeFormatResolvedOptions => ("resolvedOptions", 0),
        IntlCallable::DateTimeFormatFormatGet => ("get format", 0),
        IntlCallable::DateTimeFormatFormat(_) => ("", 1),
        IntlCallable::DateTimeFormatFormatToParts => ("formatToParts", 1),
        IntlCallable::DateTimeFormatFormatRange => ("formatRange", 2),
        IntlCallable::DateTimeFormatFormatRangeToParts => ("formatRangeToParts", 2),
        IntlCallable::PluralRulesConstructor => ("PluralRules", 0),
        IntlCallable::PluralRulesSupportedLocalesOf => ("supportedLocalesOf", 1),
        IntlCallable::PluralRulesResolvedOptions => ("resolvedOptions", 0),
        IntlCallable::PluralRulesSelect => ("select", 1),
        IntlCallable::PluralRulesSelectRange => ("selectRange", 2),
        IntlCallable::ListFormatConstructor => ("ListFormat", 0),
        IntlCallable::ListFormatSupportedLocalesOf => ("supportedLocalesOf", 1),
        IntlCallable::ListFormatResolvedOptions => ("resolvedOptions", 0),
        IntlCallable::ListFormatFormat => ("format", 1),
        IntlCallable::ListFormatFormatToParts => ("formatToParts", 1),
        IntlCallable::RelativeTimeFormatConstructor => ("RelativeTimeFormat", 0),
        IntlCallable::RelativeTimeFormatSupportedLocalesOf => ("supportedLocalesOf", 1),
        IntlCallable::RelativeTimeFormatResolvedOptions => ("resolvedOptions", 0),
        IntlCallable::RelativeTimeFormatFormat => ("format", 2),
        IntlCallable::RelativeTimeFormatFormatToParts => ("formatToParts", 2),
        IntlCallable::DisplayNamesConstructor => ("DisplayNames", 2),
        IntlCallable::DisplayNamesSupportedLocalesOf => ("supportedLocalesOf", 1),
        IntlCallable::DisplayNamesResolvedOptions => ("resolvedOptions", 0),
        IntlCallable::DisplayNamesOf => ("of", 1),
        IntlCallable::SegmenterConstructor => ("Segmenter", 0),
        IntlCallable::SegmenterSupportedLocalesOf => ("supportedLocalesOf", 1),
        IntlCallable::SegmenterResolvedOptions => ("resolvedOptions", 0),
        IntlCallable::SegmenterSegment => ("segment", 1),
        IntlCallable::SegmentsContaining => ("containing", 1),
        IntlCallable::SegmentIteratorNext => ("next", 0),
        IntlCallable::DurationFormatConstructor => ("DurationFormat", 0),
        IntlCallable::DurationFormatSupportedLocalesOf => ("supportedLocalesOf", 1),
        IntlCallable::DurationFormatResolvedOptions => ("resolvedOptions", 0),
        IntlCallable::DurationFormatFormat => ("format", 1),
        IntlCallable::DurationFormatFormatToParts => ("formatToParts", 1),
        IntlCallable::LocaleLanguage => ("get language", 0),
        IntlCallable::LocaleScript => ("get script", 0),
        IntlCallable::LocaleRegion => ("get region", 0),
        IntlCallable::LocaleBaseName => ("get baseName", 0),
        IntlCallable::LocaleCalendar => ("get calendar", 0),
        IntlCallable::LocaleCollation => ("get collation", 0),
        IntlCallable::LocaleHourCycle => ("get hourCycle", 0),
        IntlCallable::LocaleCaseFirst => ("get caseFirst", 0),
        IntlCallable::LocaleNumeric => ("get numeric", 0),
        IntlCallable::LocaleNumberingSystem => ("get numberingSystem", 0),
        IntlCallable::LocaleFirstDayOfWeek => ("get firstDayOfWeek", 0),
        IntlCallable::LocaleVariants => ("get variants", 0),
        IntlCallable::SegmentsIterator => ("[Symbol.iterator]", 0),
        IntlCallable::StringNormalize => ("normalize", 0),
        IntlCallable::StringToLowerCase => ("toLowerCase", 0),
        IntlCallable::StringToUpperCase => ("toUpperCase", 0),
        IntlCallable::StringToLocaleLowerCase => ("toLocaleLowerCase", 0),
        IntlCallable::StringToLocaleUpperCase => ("toLocaleUpperCase", 0),
        IntlCallable::StringLocaleCompare => ("localeCompare", 1),
        IntlCallable::NumberToLocaleString | IntlCallable::BigIntToLocaleString => {
            ("toLocaleString", 0)
        }
        IntlCallable::ArrayToLocaleString => ("toLocaleString", 0),
    })
}

pub(crate) fn is_constructor(kind: IntlCallable) -> bool {
    matches!(
        kind,
        IntlCallable::LocaleConstructor
            | IntlCallable::CollatorConstructor
            | IntlCallable::NumberFormatConstructor
            | IntlCallable::DateTimeFormatConstructor
            | IntlCallable::PluralRulesConstructor
            | IntlCallable::ListFormatConstructor
            | IntlCallable::RelativeTimeFormatConstructor
            | IntlCallable::DisplayNamesConstructor
            | IntlCallable::SegmenterConstructor
            | IntlCallable::DurationFormatConstructor
    )
}

pub(crate) fn incompatible(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    type_error(ctx, state, "Method called on incompatible receiver")
}

pub(crate) fn intern(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    text: impl Into<String>,
) -> i64 {
    state
        .intern_text(text.into(), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}
