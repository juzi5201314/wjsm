//! `Intl` 命名空间与构造器原型上的属性安装。

use wjsm_ir::{constants, value, wk_symbol};

use super::IntlCallable;
use crate::{BUILTIN_PROTOTYPE_PROPERTY_FLAGS, NativeAgentState, NativeCallableKind, PropertyKey};

const CONFIGURABLE: u32 = constants::FLAG_CONFIGURABLE as u32;

pub(crate) fn ensure_intl_object(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(object) = state.intl.object {
        return Some(object);
    }
    let object = state.allocate_object(16, false).ok()?;
    install_to_string_tag(state, object, "Intl").ok()?;
    install_method(
        state,
        object,
        "getCanonicalLocales",
        IntlCallable::GetCanonicalLocales,
    )
    .ok()?;
    install_method(
        state,
        object,
        "supportedValuesOf",
        IntlCallable::SupportedValuesOf,
    )
    .ok()?;
    for (name, kind) in CONSTRUCTORS {
        let constructor = state.native_callable(NativeCallableKind::Intl(*kind))?;
        attach_function_prototype(state, constructor);
        install_data_property(
            state,
            object,
            name,
            constructor,
            BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
        )
        .ok()?;
        install_supported_locales_of(state, constructor, *kind).ok()?;
    }
    state.intl.object = Some(object);
    Some(object)
}

pub(crate) fn ensure_constructor_prototype(
    state: &mut NativeAgentState,
    constructor: i64,
    kind: IntlCallable,
) -> Option<i64> {
    if let Some(prototype) = cached_prototype(state, kind) {
        return Some(prototype);
    }
    let prototype = state.allocate_object(8, false).ok()?;
    let constructor_key = state.intern_property_string("constructor".into())?;
    state
        .gc
        .heap()
        .define_data_property(
            value::decode_handle(prototype),
            constructor_key,
            constructor as u64,
            BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
        )
        .ok()?;
    install_prototype_members(state, prototype, kind).ok()?;
    cache_prototype(state, kind, prototype);
    Some(prototype)
}

pub(super) fn install_data_property(
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
    stored: i64,
    flags: u32,
) -> Result<(), ()> {
    let key = state.intern_property_string(name.into()).ok_or(())?;
    if value::is_callable(object) {
        state.callable_properties.insert((object, key), stored);
        state.callable_property_flags.insert((object, key), flags);
        return Ok(());
    }
    state
        .gc
        .heap()
        .define_data_property(value::decode_handle(object), key, stored as u64, flags)
        .map_err(|_| ())
}

pub(super) fn install_method(
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
    kind: IntlCallable,
) -> Result<(), ()> {
    let callable = state
        .native_callable(NativeCallableKind::Intl(kind))
        .ok_or(())?;
    attach_function_prototype(state, callable);
    install_data_property(
        state,
        object,
        name,
        callable,
        BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
    )
}

pub(super) fn install_accessor(
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
    getter: IntlCallable,
) -> Result<(), ()> {
    let getter = state
        .native_callable(NativeCallableKind::Intl(getter))
        .ok_or(())?;
    attach_function_prototype(state, getter);
    let key = state.intern_property_string(name.into()).ok_or(())?;
    if value::is_callable(object) {
        state
            .callable_accessors
            .insert((object, key), (getter, value::encode_undefined()));
        state
            .callable_property_flags
            .insert((object, key), CONFIGURABLE);
        return Ok(());
    }
    state
        .gc
        .heap()
        .define_accessor_property_with_flags(
            value::decode_handle(object),
            key,
            getter as u64,
            value::encode_undefined() as u64,
            CONFIGURABLE,
        )
        .map_err(|_| ())
}

fn attach_function_prototype(state: &mut NativeAgentState, callable: i64) {
    if let Some(prototype) = state.native_callable(NativeCallableKind::FunctionPrototype) {
        state
            .callable_prototypes
            .entry(callable)
            .or_insert(prototype);
    }
}

pub(super) fn install_to_string_tag(
    state: &mut NativeAgentState,
    object: i64,
    tag: &str,
) -> Result<(), ()> {
    let tag = state.intern_text(tag.into(), value::TAG_STRING).ok_or(())?;
    let key = PropertyKey::symbol(wk_symbol::TO_STRING_TAG);
    state
        .gc
        .heap()
        .define_data_property(value::decode_handle(object), key, tag as u64, CONFIGURABLE)
        .map_err(|_| ())
}

fn install_supported_locales_of(
    state: &mut NativeAgentState,
    constructor: i64,
    kind: IntlCallable,
) -> Result<(), ()> {
    let Some(supported) = supported_locales_callable(kind) else {
        return Ok(());
    };
    install_method(state, constructor, "supportedLocalesOf", supported)
}

fn install_prototype_members(
    state: &mut NativeAgentState,
    prototype: i64,
    kind: IntlCallable,
) -> Result<(), ()> {
    match kind {
        IntlCallable::LocaleConstructor => {
            install_to_string_tag(state, prototype, "Intl.Locale")?;
            for (name, getter) in LOCALE_ACCESSORS {
                install_accessor(state, prototype, name, *getter)?;
            }
            for (name, method) in LOCALE_METHODS {
                install_method(state, prototype, name, *method)?;
            }
        }
        IntlCallable::CollatorConstructor => {
            install_to_string_tag(state, prototype, "Intl.Collator")?;
            install_accessor(
                state,
                prototype,
                "compare",
                IntlCallable::CollatorCompareGet,
            )?;
            install_method(
                state,
                prototype,
                "resolvedOptions",
                IntlCallable::CollatorResolvedOptions,
            )?;
        }
        IntlCallable::NumberFormatConstructor => {
            install_to_string_tag(state, prototype, "Intl.NumberFormat")?;
            install_accessor(
                state,
                prototype,
                "format",
                IntlCallable::NumberFormatFormatGet,
            )?;
            install_method(
                state,
                prototype,
                "formatToParts",
                IntlCallable::NumberFormatFormatToParts,
            )?;
            install_method(
                state,
                prototype,
                "formatRange",
                IntlCallable::NumberFormatFormatRange,
            )?;
            install_method(
                state,
                prototype,
                "formatRangeToParts",
                IntlCallable::NumberFormatFormatRangeToParts,
            )?;
            install_method(
                state,
                prototype,
                "resolvedOptions",
                IntlCallable::NumberFormatResolvedOptions,
            )?;
        }
        IntlCallable::DateTimeFormatConstructor => {
            install_to_string_tag(state, prototype, "Intl.DateTimeFormat")?;
            install_accessor(
                state,
                prototype,
                "format",
                IntlCallable::DateTimeFormatFormatGet,
            )?;
            install_method(
                state,
                prototype,
                "formatToParts",
                IntlCallable::DateTimeFormatFormatToParts,
            )?;
            install_method(
                state,
                prototype,
                "formatRange",
                IntlCallable::DateTimeFormatFormatRange,
            )?;
            install_method(
                state,
                prototype,
                "formatRangeToParts",
                IntlCallable::DateTimeFormatFormatRangeToParts,
            )?;
            install_method(
                state,
                prototype,
                "resolvedOptions",
                IntlCallable::DateTimeFormatResolvedOptions,
            )?;
        }
        IntlCallable::PluralRulesConstructor => {
            install_to_string_tag(state, prototype, "Intl.PluralRules")?;
            install_method(state, prototype, "select", IntlCallable::PluralRulesSelect)?;
            install_method(
                state,
                prototype,
                "selectRange",
                IntlCallable::PluralRulesSelectRange,
            )?;
            install_method(
                state,
                prototype,
                "resolvedOptions",
                IntlCallable::PluralRulesResolvedOptions,
            )?;
        }
        IntlCallable::ListFormatConstructor => {
            install_to_string_tag(state, prototype, "Intl.ListFormat")?;
            install_method(state, prototype, "format", IntlCallable::ListFormatFormat)?;
            install_method(
                state,
                prototype,
                "formatToParts",
                IntlCallable::ListFormatFormatToParts,
            )?;
            install_method(
                state,
                prototype,
                "resolvedOptions",
                IntlCallable::ListFormatResolvedOptions,
            )?;
        }
        IntlCallable::RelativeTimeFormatConstructor => {
            install_to_string_tag(state, prototype, "Intl.RelativeTimeFormat")?;
            install_method(
                state,
                prototype,
                "format",
                IntlCallable::RelativeTimeFormatFormat,
            )?;
            install_method(
                state,
                prototype,
                "formatToParts",
                IntlCallable::RelativeTimeFormatFormatToParts,
            )?;
            install_method(
                state,
                prototype,
                "resolvedOptions",
                IntlCallable::RelativeTimeFormatResolvedOptions,
            )?;
        }
        IntlCallable::DisplayNamesConstructor => {
            install_to_string_tag(state, prototype, "Intl.DisplayNames")?;
            install_method(state, prototype, "of", IntlCallable::DisplayNamesOf)?;
            install_method(
                state,
                prototype,
                "resolvedOptions",
                IntlCallable::DisplayNamesResolvedOptions,
            )?;
        }
        IntlCallable::SegmenterConstructor => {
            install_to_string_tag(state, prototype, "Intl.Segmenter")?;
            install_method(state, prototype, "segment", IntlCallable::SegmenterSegment)?;
            install_method(
                state,
                prototype,
                "resolvedOptions",
                IntlCallable::SegmenterResolvedOptions,
            )?;
        }
        IntlCallable::DurationFormatConstructor => {
            install_to_string_tag(state, prototype, "Intl.DurationFormat")?;
            install_method(
                state,
                prototype,
                "format",
                IntlCallable::DurationFormatFormat,
            )?;
            install_method(
                state,
                prototype,
                "formatToParts",
                IntlCallable::DurationFormatFormatToParts,
            )?;
            install_method(
                state,
                prototype,
                "resolvedOptions",
                IntlCallable::DurationFormatResolvedOptions,
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn supported_locales_callable(kind: IntlCallable) -> Option<IntlCallable> {
    Some(match kind {
        IntlCallable::CollatorConstructor => IntlCallable::CollatorSupportedLocalesOf,
        IntlCallable::NumberFormatConstructor => IntlCallable::NumberFormatSupportedLocalesOf,
        IntlCallable::DateTimeFormatConstructor => IntlCallable::DateTimeFormatSupportedLocalesOf,
        IntlCallable::PluralRulesConstructor => IntlCallable::PluralRulesSupportedLocalesOf,
        IntlCallable::ListFormatConstructor => IntlCallable::ListFormatSupportedLocalesOf,
        IntlCallable::RelativeTimeFormatConstructor => {
            IntlCallable::RelativeTimeFormatSupportedLocalesOf
        }
        IntlCallable::DisplayNamesConstructor => IntlCallable::DisplayNamesSupportedLocalesOf,
        IntlCallable::SegmenterConstructor => IntlCallable::SegmenterSupportedLocalesOf,
        IntlCallable::DurationFormatConstructor => IntlCallable::DurationFormatSupportedLocalesOf,
        _ => return None,
    })
}

fn cached_prototype(state: &NativeAgentState, kind: IntlCallable) -> Option<i64> {
    match kind {
        IntlCallable::LocaleConstructor => state.intl.locale_prototype,
        IntlCallable::CollatorConstructor => state.intl.collator_prototype,
        IntlCallable::NumberFormatConstructor => state.intl.number_format_prototype,
        IntlCallable::DateTimeFormatConstructor => state.intl.datetime_format_prototype,
        IntlCallable::PluralRulesConstructor => state.intl.plural_rules_prototype,
        IntlCallable::ListFormatConstructor => state.intl.list_format_prototype,
        IntlCallable::RelativeTimeFormatConstructor => state.intl.relative_time_prototype,
        IntlCallable::DisplayNamesConstructor => state.intl.display_names_prototype,
        IntlCallable::SegmenterConstructor => state.intl.segmenter_prototype,
        IntlCallable::DurationFormatConstructor => state.intl.duration_format_prototype,
        _ => None,
    }
}

fn cache_prototype(state: &mut NativeAgentState, kind: IntlCallable, prototype: i64) {
    match kind {
        IntlCallable::LocaleConstructor => state.intl.locale_prototype = Some(prototype),
        IntlCallable::CollatorConstructor => state.intl.collator_prototype = Some(prototype),
        IntlCallable::NumberFormatConstructor => {
            state.intl.number_format_prototype = Some(prototype)
        }
        IntlCallable::DateTimeFormatConstructor => {
            state.intl.datetime_format_prototype = Some(prototype)
        }
        IntlCallable::PluralRulesConstructor => state.intl.plural_rules_prototype = Some(prototype),
        IntlCallable::ListFormatConstructor => state.intl.list_format_prototype = Some(prototype),
        IntlCallable::RelativeTimeFormatConstructor => {
            state.intl.relative_time_prototype = Some(prototype)
        }
        IntlCallable::DisplayNamesConstructor => {
            state.intl.display_names_prototype = Some(prototype)
        }
        IntlCallable::SegmenterConstructor => state.intl.segmenter_prototype = Some(prototype),
        IntlCallable::DurationFormatConstructor => {
            state.intl.duration_format_prototype = Some(prototype)
        }
        _ => {}
    }
}

const CONSTRUCTORS: &[(&str, IntlCallable)] = &[
    ("Locale", IntlCallable::LocaleConstructor),
    ("Collator", IntlCallable::CollatorConstructor),
    ("NumberFormat", IntlCallable::NumberFormatConstructor),
    ("DateTimeFormat", IntlCallable::DateTimeFormatConstructor),
    ("PluralRules", IntlCallable::PluralRulesConstructor),
    ("ListFormat", IntlCallable::ListFormatConstructor),
    (
        "RelativeTimeFormat",
        IntlCallable::RelativeTimeFormatConstructor,
    ),
    ("DisplayNames", IntlCallable::DisplayNamesConstructor),
    ("Segmenter", IntlCallable::SegmenterConstructor),
    ("DurationFormat", IntlCallable::DurationFormatConstructor),
];

const LOCALE_ACCESSORS: &[(&str, IntlCallable)] = &[
    ("language", IntlCallable::LocaleLanguage),
    ("script", IntlCallable::LocaleScript),
    ("region", IntlCallable::LocaleRegion),
    ("baseName", IntlCallable::LocaleBaseName),
    ("calendar", IntlCallable::LocaleCalendar),
    ("collation", IntlCallable::LocaleCollation),
    ("hourCycle", IntlCallable::LocaleHourCycle),
    ("caseFirst", IntlCallable::LocaleCaseFirst),
    ("numeric", IntlCallable::LocaleNumeric),
    ("numberingSystem", IntlCallable::LocaleNumberingSystem),
    ("firstDayOfWeek", IntlCallable::LocaleFirstDayOfWeek),
    ("variants", IntlCallable::LocaleVariants),
];

const LOCALE_METHODS: &[(&str, IntlCallable)] = &[
    ("maximize", IntlCallable::LocaleMaximize),
    ("minimize", IntlCallable::LocaleMinimize),
    ("toString", IntlCallable::LocaleToString),
    ("getCalendars", IntlCallable::LocaleGetCalendars),
    ("getCollations", IntlCallable::LocaleGetCollations),
    ("getHourCycles", IntlCallable::LocaleGetHourCycles),
    (
        "getNumberingSystems",
        IntlCallable::LocaleGetNumberingSystems,
    ),
    ("getTimeZones", IntlCallable::LocaleGetTimeZones),
    ("getTextInfo", IntlCallable::LocaleGetTextInfo),
    ("getWeekInfo", IntlCallable::LocaleGetWeekInfo),
];
