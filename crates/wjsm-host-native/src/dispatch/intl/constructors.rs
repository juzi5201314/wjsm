//! Intl 可调用分派。

use wjsm_builtins::intl::{get_canonical_locales, supported_values_of};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::common::throw_intl;
use super::js::{canonicalize_locales, string_array};
use super::{
    IntlCallable, collator, datetime_format, display_names, duration_format, list_format, locale,
    locale_methods, number_format, plural_rules, relative_time, segmenter,
};
use crate::NativeAgentState;
use crate::dispatch::runtime::to_string_coerced;

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: IntlCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    match callable {
        IntlCallable::GetCanonicalLocales => get_canonical(ctx, state, args),
        IntlCallable::SupportedValuesOf => supported_values(ctx, state, args),
        IntlCallable::LocaleConstructor
        | IntlCallable::LocaleMaximize
        | IntlCallable::LocaleMinimize
        | IntlCallable::LocaleToString
        | IntlCallable::LocaleGetCalendars
        | IntlCallable::LocaleGetCollations
        | IntlCallable::LocaleGetHourCycles
        | IntlCallable::LocaleGetNumberingSystems
        | IntlCallable::LocaleGetTimeZones
        | IntlCallable::LocaleGetTextInfo
        | IntlCallable::LocaleGetWeekInfo
        | IntlCallable::LocaleLanguage
        | IntlCallable::LocaleScript
        | IntlCallable::LocaleRegion
        | IntlCallable::LocaleBaseName
        | IntlCallable::LocaleCalendar
        | IntlCallable::LocaleCollation
        | IntlCallable::LocaleHourCycle
        | IntlCallable::LocaleCaseFirst
        | IntlCallable::LocaleNumeric
        | IntlCallable::LocaleNumberingSystem
        | IntlCallable::LocaleFirstDayOfWeek
        | IntlCallable::LocaleVariants => locale::call(ctx, state, callable, receiver, args),
        IntlCallable::CollatorConstructor
        | IntlCallable::CollatorSupportedLocalesOf
        | IntlCallable::CollatorResolvedOptions
        | IntlCallable::CollatorCompareGet
        | IntlCallable::CollatorCompare(_) => collator::call(ctx, state, callable, receiver, args),
        IntlCallable::NumberFormatConstructor
        | IntlCallable::NumberFormatSupportedLocalesOf
        | IntlCallable::NumberFormatResolvedOptions
        | IntlCallable::NumberFormatFormatGet
        | IntlCallable::NumberFormatFormat(_)
        | IntlCallable::NumberFormatFormatToParts
        | IntlCallable::NumberFormatFormatRange
        | IntlCallable::NumberFormatFormatRangeToParts => {
            number_format::call(ctx, state, callable, receiver, args)
        }
        IntlCallable::DateTimeFormatConstructor
        | IntlCallable::DateTimeFormatSupportedLocalesOf
        | IntlCallable::DateTimeFormatResolvedOptions
        | IntlCallable::DateTimeFormatFormatGet
        | IntlCallable::DateTimeFormatFormat(_)
        | IntlCallable::DateTimeFormatFormatToParts
        | IntlCallable::DateTimeFormatFormatRange
        | IntlCallable::DateTimeFormatFormatRangeToParts => {
            datetime_format::call(ctx, state, callable, receiver, args)
        }
        IntlCallable::PluralRulesConstructor
        | IntlCallable::PluralRulesSupportedLocalesOf
        | IntlCallable::PluralRulesResolvedOptions
        | IntlCallable::PluralRulesSelect
        | IntlCallable::PluralRulesSelectRange => {
            plural_rules::call(ctx, state, callable, receiver, args)
        }
        IntlCallable::ListFormatConstructor
        | IntlCallable::ListFormatSupportedLocalesOf
        | IntlCallable::ListFormatResolvedOptions
        | IntlCallable::ListFormatFormat
        | IntlCallable::ListFormatFormatToParts => {
            list_format::call(ctx, state, callable, receiver, args)
        }
        IntlCallable::RelativeTimeFormatConstructor
        | IntlCallable::RelativeTimeFormatSupportedLocalesOf
        | IntlCallable::RelativeTimeFormatResolvedOptions
        | IntlCallable::RelativeTimeFormatFormat
        | IntlCallable::RelativeTimeFormatFormatToParts => {
            relative_time::call(ctx, state, callable, receiver, args)
        }
        IntlCallable::DisplayNamesConstructor
        | IntlCallable::DisplayNamesSupportedLocalesOf
        | IntlCallable::DisplayNamesResolvedOptions
        | IntlCallable::DisplayNamesOf => display_names::call(ctx, state, callable, receiver, args),
        IntlCallable::SegmenterConstructor
        | IntlCallable::SegmenterSupportedLocalesOf
        | IntlCallable::SegmenterResolvedOptions
        | IntlCallable::SegmenterSegment
        | IntlCallable::SegmentsContaining
        | IntlCallable::SegmentsIterator
        | IntlCallable::SegmentIteratorNext => {
            segmenter::call(ctx, state, callable, receiver, args)
        }
        IntlCallable::DurationFormatConstructor
        | IntlCallable::DurationFormatSupportedLocalesOf
        | IntlCallable::DurationFormatResolvedOptions
        | IntlCallable::DurationFormatFormat
        | IntlCallable::DurationFormatFormatToParts => {
            duration_format::call(ctx, state, callable, receiver, args)
        }
        IntlCallable::StringNormalize
        | IntlCallable::StringToLowerCase
        | IntlCallable::StringToUpperCase
        | IntlCallable::StringToLocaleLowerCase
        | IntlCallable::StringToLocaleUpperCase
        | IntlCallable::StringLocaleCompare
        | IntlCallable::NumberToLocaleString
        | IntlCallable::BigIntToLocaleString
        | IntlCallable::ArrayToLocaleString => {
            locale_methods::call(ctx, state, callable, receiver, args)
        }
    }
}

fn get_canonical(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let locales = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    match canonicalize_locales(ctx, state, locales) {
        Ok(tags) => match get_canonical_locales(&tags) {
            Ok(canonical) => string_array(ctx, state, &canonical),
            Err(error) => throw_intl(ctx, state, error),
        },
        Err(exception) => exception,
    }
}

fn supported_values(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let key = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let key = match to_string_coerced(ctx, state, key) {
        Ok(key) => key,
        Err(exception) => return exception,
    };
    match supported_values_of(&key) {
        Ok(values) => string_array(ctx, state, &values),
        Err(error) => throw_intl(ctx, state, error),
    }
}
