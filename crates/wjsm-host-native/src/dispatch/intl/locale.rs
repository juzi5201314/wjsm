//! `Intl.Locale` 构造器与 Locale-info。

use wjsm_builtins::intl::canonicalize_unicode_locale_id;
use wjsm_intl_data::{
    available_calendars, available_collations, available_numbering_systems, available_time_zones,
    expand_likely_subtags, minimize_likely_subtags,
};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::common::{create_instance, intern, is_type_object, slot_handle, throw_intl};
use super::js::{
    get_option_bool_opt, get_option_string, get_options_object, is_locale_object, to_object,
};
use super::slots::{IntlSlot, LocaleSlot};
use super::{IntlCallable, incompatible};
use crate::NativeAgentState;
use crate::dispatch::runtime::{fail_dispatch, to_string_coerced, type_error};

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: IntlCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    match callable {
        IntlCallable::LocaleConstructor => construct(ctx, state, receiver, args),
        IntlCallable::LocaleToString => locale_string(ctx, state, receiver, false),
        IntlCallable::LocaleMaximize => transform(ctx, state, receiver, true),
        IntlCallable::LocaleMinimize => transform(ctx, state, receiver, false),
        IntlCallable::LocaleLanguage => field(ctx, state, receiver, Field::Language),
        IntlCallable::LocaleScript => field(ctx, state, receiver, Field::Script),
        IntlCallable::LocaleRegion => field(ctx, state, receiver, Field::Region),
        IntlCallable::LocaleBaseName => field(ctx, state, receiver, Field::BaseName),
        IntlCallable::LocaleCalendar => field(ctx, state, receiver, Field::Calendar),
        IntlCallable::LocaleCollation => field(ctx, state, receiver, Field::Collation),
        IntlCallable::LocaleHourCycle => field(ctx, state, receiver, Field::HourCycle),
        IntlCallable::LocaleCaseFirst => field(ctx, state, receiver, Field::CaseFirst),
        IntlCallable::LocaleNumeric => field(ctx, state, receiver, Field::Numeric),
        IntlCallable::LocaleNumberingSystem => field(ctx, state, receiver, Field::NumberingSystem),
        IntlCallable::LocaleFirstDayOfWeek => field(ctx, state, receiver, Field::FirstDayOfWeek),
        IntlCallable::LocaleVariants => field(ctx, state, receiver, Field::Variants),
        IntlCallable::LocaleGetCalendars => {
            string_list(ctx, state, receiver, available_calendars())
        }
        IntlCallable::LocaleGetCollations => {
            string_list(ctx, state, receiver, available_collations())
        }
        IntlCallable::LocaleGetHourCycles => {
            string_list(ctx, state, receiver, &["h12", "h23", "h11", "h24"])
        }
        IntlCallable::LocaleGetNumberingSystems => {
            string_list(ctx, state, receiver, available_numbering_systems())
        }
        IntlCallable::LocaleGetTimeZones => time_zones(ctx, state, receiver),
        IntlCallable::LocaleGetTextInfo => text_info(ctx, state, receiver),
        IntlCallable::LocaleGetWeekInfo => week_info(ctx, state, receiver),
        _ => fail_dispatch(ctx),
    }
}

fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    if value::is_undefined(super::common::current_new_target(state)) {
        return type_error(ctx, state, "Intl.Locale must be called with new");
    }
    let tag = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_undefined(tag) {
        return type_error(ctx, state, "Intl.Locale requires a tag");
    }
    if !value::is_string(tag) && !is_locale_object(state, tag) && !is_type_object(tag) {
        return type_error(ctx, state, "Intl.Locale tag must be a string or object");
    }
    let mut tag = if is_locale_object(state, tag) {
        match state.intl.slots.get(&value::decode_handle(tag)) {
            Some(IntlSlot::Locale(slot)) => slot.tag.clone(),
            _ => return fail_dispatch(ctx),
        }
    } else {
        match to_string_coerced(ctx, state, tag) {
            Ok(tag) => tag,
            Err(exception) => return exception,
        }
    };
    let canonical = match canonicalize_unicode_locale_id(&tag) {
        Ok(canonical) => canonical,
        Err(error) => return throw_intl(ctx, state, error),
    };
    tag = canonical;
    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let options = if value::is_undefined(options) {
        match get_options_object(ctx, state, options) {
            Ok(options) => options,
            Err(exception) => return exception,
        }
    } else {
        match to_object(ctx, state, options) {
            Ok(options) => options,
            Err(exception) => return exception,
        }
    };
    let slot = match apply_options(ctx, state, tag, options) {
        Ok(slot) => slot,
        Err(exception) => return exception,
    };
    create_instance(
        ctx,
        state,
        IntlCallable::LocaleConstructor,
        IntlSlot::Locale(slot),
        this_value,
    )
}

fn apply_options(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    tag: String,
    options: i64,
) -> Result<LocaleSlot, i64> {
    let language = get_option_string(ctx, state, options, "language", &[], None)?;
    let language = require_subtag(ctx, state, language, is_language_subtag)?;
    let script = get_option_string(ctx, state, options, "script", &[], None)?;
    let script = require_subtag(ctx, state, script, is_script_subtag)?;
    let region = get_option_string(ctx, state, options, "region", &[], None)?;
    let region = require_subtag(ctx, state, region, is_region_subtag)?;
    let variants_option = get_option_string(ctx, state, options, "variants", &[], None)?;
    let variants_option = require_subtag(ctx, state, variants_option, is_variants_sequence)?;
    let calendar = get_option_string(ctx, state, options, "calendar", &[], None)?;
    let mut calendar = require_subtag(ctx, state, calendar, is_unicode_type_sequence)?;
    let collation = get_option_string(ctx, state, options, "collation", &[], None)?;
    let collation = require_subtag(ctx, state, collation, is_unicode_type_sequence)?;
    let first_day_of_week =
        match get_option_string(ctx, state, options, "firstDayOfWeek", &[], None)? {
            Some(value) => Some(weekday_to_string(ctx, state, &value)?),
            None => None,
        };
    let hour_cycle = get_option_string(
        ctx,
        state,
        options,
        "hourCycle",
        &["h11", "h12", "h23", "h24"],
        None,
    )?;
    let case_first = get_option_string(
        ctx,
        state,
        options,
        "caseFirst",
        &["upper", "lower", "false"],
        None,
    )?;
    let numeric = get_option_bool_opt(ctx, state, options, "numeric")?;
    let numbering_system = get_option_string(ctx, state, options, "numberingSystem", &[], None)?;
    let numbering_system = require_subtag(ctx, state, numbering_system, is_unicode_type_sequence)?;
    let mut fields = wjsm_intl_data::tag::language_fields(&tag).ok_or_else(|| {
        crate::dispatch::runtime::range_error(ctx, state, &format!("Invalid language tag: {tag}"))
    })?;
    if let Some(language) = &language {
        fields.language = language.to_ascii_lowercase();
    }
    if let Some(script) = &script {
        fields.script = Some(script.clone());
    }
    if let Some(region) = &region {
        fields.region = Some(region.clone());
    }
    if let Some(value) = variants_option {
        fields.variants = value
            .split('-')
            .filter(|part| !part.is_empty())
            .map(|part| part.to_ascii_lowercase())
            .collect();
    }
    let mut keywords = wjsm_intl_data::tag::unicode_keywords(&tag);
    if let Some(calendar) = calendar.as_mut() {
        *calendar = wjsm_intl_data::aliases::canonicalize_unicode_keyword("ca", calendar);
        keywords.insert("ca".into(), calendar.clone());
    }
    if let Some(collation) = &collation {
        keywords.insert("co".into(), collation.to_ascii_lowercase());
    }
    if let Some(hour_cycle) = &hour_cycle {
        keywords.insert("hc".into(), hour_cycle.to_ascii_lowercase());
    }
    if let Some(case_first) = &case_first {
        keywords.insert("kf".into(), case_first.to_ascii_lowercase());
    }
    if let Some(numbering_system) = &numbering_system {
        keywords.insert("nu".into(), numbering_system.to_ascii_lowercase());
    }
    if let Some(numeric) = numeric {
        keywords.insert(
            "kn".into(),
            if numeric {
                String::new()
            } else {
                "false".into()
            },
        );
    }
    if let Some(first_day_of_week) = &first_day_of_week {
        keywords.insert("fw".into(), first_day_of_week.clone());
    }
    let mut case_first = case_first.or_else(|| keyword(&keywords, "kf"));
    if case_first.as_deref() == Some("true") {
        case_first = Some(String::new());
        keywords.insert("kf".into(), String::new());
    }
    let attributes = wjsm_intl_data::tag::unicode_attributes(&tag);
    let rebuilt = wjsm_intl_data::tag::format_unicode_locale(
        &fields.language,
        fields.script.as_deref(),
        fields.region.as_deref(),
        &fields.variants,
        &attributes,
        &keywords,
        &tag,
    );
    let canonical =
        canonicalize_unicode_locale_id(&rebuilt).map_err(|error| throw_intl(ctx, state, error))?;
    slot_from_canonical(
        canonical,
        LocaleOverrides {
            calendar,
            collation,
            hour_cycle,
            case_first,
            numbering_system,
            numeric,
            first_day_of_week,
        },
    )
    .ok_or_else(|| crate::dispatch::runtime::range_error(ctx, state, "Invalid language tag"))
}

fn field(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    field: Field,
) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::Locale(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    if matches!(field, Field::Numeric) {
        return value::encode_bool(slot.numeric);
    }
    let text = match field {
        Field::Language => Some(slot.language.clone()),
        Field::Script => slot.script.clone(),
        Field::Region => slot.region.clone(),
        Field::BaseName => Some(base_name(slot)),
        Field::Calendar => slot.calendar.clone(),
        Field::Collation => slot.collation.clone(),
        Field::HourCycle => slot.hour_cycle.clone(),
        Field::CaseFirst => slot.case_first.clone(),
        Field::NumberingSystem => slot.numbering_system.clone(),
        Field::FirstDayOfWeek => slot.first_day_of_week.clone(),
        Field::Variants => {
            if slot.variants.is_empty() {
                None
            } else {
                Some(slot.variants.join("-"))
            }
        }
        Field::Numeric => unreachable!(),
    };
    optional(ctx, state, text.as_deref())
}

fn locale_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    _unused: bool,
) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::Locale(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let tag = slot.tag.clone();
    intern(ctx, state, tag)
}

fn transform(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    maximize: bool,
) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::Locale(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let tag = slot.tag.clone();
    let base = base_name(slot);
    let rest = tag.strip_prefix(&base).unwrap_or("").to_owned();
    let transformed = if maximize {
        expand_likely_subtags(&base)
    } else {
        minimize_likely_subtags(&base)
    };
    let Ok(transformed) = transformed else {
        return create_from_tag(ctx, state, tag);
    };
    create_from_tag(ctx, state, format!("{transformed}{rest}"))
}

fn create_from_tag(ctx: &mut NativeVmContext, state: &mut NativeAgentState, tag: String) -> i64 {
    let Some(slot) = slot_from_canonical(tag, LocaleOverrides::default()) else {
        return fail_dispatch(ctx);
    };
    create_instance(
        ctx,
        state,
        IntlCallable::LocaleConstructor,
        IntlSlot::Locale(slot),
        value::encode_undefined(),
    )
}

fn string_list(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    items: &[&str],
) -> i64 {
    if !is_locale_receiver(state, receiver) {
        return incompatible(ctx, state);
    }
    super::js::string_array(
        ctx,
        state,
        &items
            .iter()
            .map(|item| (*item).to_owned())
            .collect::<Vec<_>>(),
    )
}

fn time_zones(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let has_region = match state.intl.slots.get(&handle) {
        Some(IntlSlot::Locale(slot)) => slot.region.is_some(),
        _ => return incompatible(ctx, state),
    };
    if !has_region {
        return value::encode_undefined();
    }
    super::js::string_array(ctx, state, available_time_zones())
}

fn text_info(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let rtl = match state.intl.slots.get(&handle) {
        Some(IntlSlot::Locale(slot)) => {
            matches!(
                slot.language.as_str(),
                "ar" | "fa" | "he" | "ur" | "ps" | "yi"
            )
        }
        _ => return incompatible(ctx, state),
    };
    let object = match state.allocate_object(1, false) {
        Ok(object) => object,
        Err(_) => return fail_dispatch(ctx),
    };
    let direction = intern(ctx, state, if rtl { "rtl" } else { "ltr" });
    if let Err(exception) = super::common::set_data(ctx, state, object, "direction", direction) {
        return exception;
    }
    object
}

fn week_info(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    if !is_locale_receiver(state, receiver) {
        return incompatible(ctx, state);
    }
    let first_day = slot_handle(receiver)
        .and_then(|handle| state.intl.slots.get(&handle))
        .and_then(|slot| match slot {
            IntlSlot::Locale(slot) => slot.first_day_of_week.as_deref(),
            _ => None,
        })
        .map(weekday_number)
        .unwrap_or(1.0);
    let object = match state.allocate_object(2, false) {
        Ok(object) => object,
        Err(_) => return fail_dispatch(ctx),
    };
    let weekend =
        match state.allocate_array_values(&[value::encode_f64(6.0), value::encode_f64(7.0)]) {
            Ok(weekend) => weekend,
            Err(_) => return fail_dispatch(ctx),
        };
    for (name, stored) in [
        ("firstDay", value::encode_f64(first_day)),
        ("weekend", weekend),
    ] {
        if let Err(exception) = super::common::set_data(ctx, state, object, name, stored) {
            return exception;
        }
    }
    object
}

fn is_locale_receiver(state: &NativeAgentState, receiver: i64) -> bool {
    slot_handle(receiver)
        .is_some_and(|handle| matches!(state.intl.slots.get(&handle), Some(IntlSlot::Locale(_))))
}

fn optional(ctx: &mut NativeVmContext, state: &mut NativeAgentState, value: Option<&str>) -> i64 {
    match value {
        Some(value) => intern(ctx, state, value.to_owned()),
        None => value::encode_undefined(),
    }
}

#[derive(Default)]
struct LocaleOverrides {
    calendar: Option<String>,
    collation: Option<String>,
    hour_cycle: Option<String>,
    case_first: Option<String>,
    numbering_system: Option<String>,
    numeric: Option<bool>,
    first_day_of_week: Option<String>,
}

fn slot_from_canonical(tag: String, overrides: LocaleOverrides) -> Option<LocaleSlot> {
    let fields = wjsm_intl_data::tag::language_fields(&tag)?;
    let keywords = wjsm_intl_data::tag::unicode_keywords(&tag);
    let mut case_first = overrides.case_first.or_else(|| keyword(&keywords, "kf"));
    if case_first.as_deref() == Some("true") {
        case_first = Some(String::new());
    }
    Some(LocaleSlot {
        language: fields.language,
        script: fields.script,
        region: fields.region,
        variants: fields.variants,
        calendar: overrides.calendar.or_else(|| keyword(&keywords, "ca")),
        collation: overrides.collation.or_else(|| keyword(&keywords, "co")),
        hour_cycle: overrides.hour_cycle.or_else(|| keyword(&keywords, "hc")),
        case_first,
        numbering_system: overrides
            .numbering_system
            .or_else(|| keyword(&keywords, "nu")),
        numeric: overrides
            .numeric
            .unwrap_or_else(|| keywords.get("kn").is_some_and(|value| value != "false")),
        first_day_of_week: overrides
            .first_day_of_week
            .or_else(|| keyword(&keywords, "fw")),
        tag,
    })
}

fn base_name(slot: &LocaleSlot) -> String {
    let mut tag = slot.language.clone();
    if let Some(script) = &slot.script {
        tag.push('-');
        tag.push_str(script);
    }
    if let Some(region) = &slot.region {
        tag.push('-');
        tag.push_str(region);
    }
    for variant in &slot.variants {
        tag.push('-');
        tag.push_str(variant);
    }
    tag
}

enum Field {
    Language,
    Script,
    Region,
    BaseName,
    Calendar,
    Collation,
    HourCycle,
    CaseFirst,
    Numeric,
    NumberingSystem,
    FirstDayOfWeek,
    Variants,
}

fn keyword(keywords: &std::collections::BTreeMap<String, String>, key: &str) -> Option<String> {
    keywords.get(key).map(|value| {
        if value == "true" {
            String::new()
        } else {
            value.clone()
        }
    })
}

fn weekday_to_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    value: &str,
) -> Result<String, i64> {
    Ok(match value {
        "mon" | "1" => "mon".into(),
        "tue" | "2" => "tue".into(),
        "wed" | "3" => "wed".into(),
        "thu" | "4" => "thu".into(),
        "fri" | "5" => "fri".into(),
        "sat" | "6" => "sat".into(),
        "sun" | "0" | "7" => "sun".into(),
        "true" => String::new(),
        other if is_unicode_type_sequence(other) => other.to_ascii_lowercase(),
        _ => {
            return Err(crate::dispatch::runtime::range_error(
                ctx,
                state,
                "invalid firstDayOfWeek",
            ));
        }
    })
}

fn is_unicode_type_sequence(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|part| {
            (3..=8).contains(&part.len()) && part.chars().all(|ch| ch.is_ascii_alphanumeric())
        })
}

fn require_subtag(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    value: Option<String>,
    valid: impl Fn(&str) -> bool,
) -> Result<Option<String>, i64> {
    match value {
        None => Ok(None),
        Some(value) if valid(&value) => Ok(Some(value)),
        Some(_) => Err(crate::dispatch::runtime::range_error(
            ctx,
            state,
            "invalid locale option",
        )),
    }
}

fn is_language_subtag(value: &str) -> bool {
    (2..=8).contains(&value.len()) && value.chars().all(|ch| ch.is_ascii_alphabetic())
}

fn is_script_subtag(value: &str) -> bool {
    value.len() == 4 && value.chars().all(|ch| ch.is_ascii_alphabetic())
}

fn is_region_subtag(value: &str) -> bool {
    (value.len() == 2 && value.chars().all(|ch| ch.is_ascii_alphabetic()))
        || (value.len() == 3 && value.chars().all(|ch| ch.is_ascii_digit()))
}

fn is_variants_sequence(value: &str) -> bool {
    if value.is_empty() || value.starts_with('-') || value.ends_with('-') || value.contains("--") {
        return false;
    }
    let mut seen = std::collections::HashSet::new();
    value.split('-').all(|part| {
        let len = part.len();
        let valid = ((5..=8).contains(&len) && part.chars().all(|ch| ch.is_ascii_alphanumeric()))
            || (len == 4
                && part.as_bytes()[0].is_ascii_digit()
                && part.chars().all(|ch| ch.is_ascii_alphanumeric()));
        valid && seen.insert(part.to_ascii_lowercase())
    })
}

fn weekday_number(value: &str) -> f64 {
    match value {
        "mon" => 1.0,
        "tue" => 2.0,
        "wed" => 3.0,
        "thu" => 4.0,
        "fri" => 5.0,
        "sat" => 6.0,
        "sun" => 7.0,
        _ => 1.0,
    }
}
