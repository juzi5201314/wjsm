//! `Intl.Collator`。

use std::collections::BTreeMap;

use wjsm_builtins::intl::resolve_locale_filtered;
use wjsm_intl_data::{
    CollatorSensitivity, OwnedCollator, collation_supported, default_ignore_punctuation,
};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::common::{create_instance, intern, resolved_object, slot_handle, throw_intl};
use super::js::{
    canonicalize_locales, get_option_bool_opt, get_option_string, get_options_object,
    supported_locales_of,
};
use super::slots::{CollatorSlot, IntlSlot};
use super::{IntlCallable, incompatible};
use crate::NativeAgentState;
use crate::NativeCallableKind;
use crate::dispatch::runtime::{fail_dispatch, to_string_coerced};

pub(super) fn compare_with_options(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    locales: i64,
    options: i64,
    left: i64,
    right: i64,
) -> i64 {
    let instance = construct(ctx, state, value::encode_undefined(), &[locales, options]);
    if value::is_exception(instance) {
        return instance;
    }
    compare(ctx, state, value::decode_handle(instance), &[left, right])
}

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: IntlCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    match callable {
        IntlCallable::CollatorConstructor => construct(ctx, state, receiver, args),
        IntlCallable::CollatorSupportedLocalesOf => supported_locales_of(ctx, state, args),
        IntlCallable::CollatorResolvedOptions => resolved(ctx, state, receiver),
        IntlCallable::CollatorCompareGet => compare_get(ctx, state, receiver),
        IntlCallable::CollatorCompare(handle) => compare(ctx, state, handle, args),
        _ => fail_dispatch(ctx),
    }
}

fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    let locales = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let requested = match canonicalize_locales(ctx, state, locales) {
        Ok(requested) => requested,
        Err(exception) => return exception,
    };
    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let options = match get_options_object(ctx, state, options) {
        Ok(options) => options,
        Err(exception) => return exception,
    };
    let usage = match get_option_string(
        ctx,
        state,
        options,
        "usage",
        &["sort", "search"],
        Some("sort"),
    ) {
        Ok(value) => value.unwrap_or_else(|| "sort".into()),
        Err(exception) => return exception,
    };
    if let Err(exception) = get_option_string(
        ctx,
        state,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    ) {
        return exception;
    }
    let numeric = match get_option_bool_opt(ctx, state, options, "numeric") {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let case_first = match get_option_string(
        ctx,
        state,
        options,
        "caseFirst",
        &["upper", "lower", "false"],
        None,
    ) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let sensitivity = match get_option_string(
        ctx,
        state,
        options,
        "sensitivity",
        &["base", "accent", "case", "variant"],
        None,
    ) {
        Ok(value) => value.unwrap_or_else(|| "variant".into()),
        Err(exception) => return exception,
    };
    let ignore_punctuation = match get_option_bool_opt(ctx, state, options, "ignorePunctuation") {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let collation = match get_option_string(ctx, state, options, "collation", &[], None) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let mut ext = BTreeMap::new();
    if let Some(collation) = collation {
        ext.insert("co".into(), collation);
    }
    if let Some(numeric) = numeric {
        ext.insert("kn".into(), if numeric { "true" } else { "false" }.into());
    }
    if let Some(case_first) = case_first {
        ext.insert("kf".into(), case_first);
    }
    let resolved = match resolve_locale_filtered(
        &requested,
        &["co", "kn", "kf"],
        &ext,
        |key, value, data_locale| match key {
            "co" => collation_supported(data_locale, value),
            "kn" => matches!(value, "true" | "false"),
            "kf" => matches!(value, "upper" | "lower" | "false"),
            _ => false,
        },
    ) {
        Ok(resolved) => resolved,
        Err(error) => return throw_intl(ctx, state, error),
    };
    let collation = resolved
        .extensions
        .get("co")
        .cloned()
        .unwrap_or_else(|| "default".into());
    let numeric = resolved
        .extensions
        .get("kn")
        .map(|value| value != "false")
        .unwrap_or(false);
    let case_first = resolved
        .extensions
        .get("kf")
        .cloned()
        .unwrap_or_else(|| "false".into());
    let ignore_punctuation =
        ignore_punctuation.unwrap_or_else(|| default_ignore_punctuation(&resolved.locale));
    let slot = CollatorSlot {
        locale: resolved.locale,
        usage,
        sensitivity,
        ignore_punctuation,
        numeric,
        case_first,
        collation,
        bound_compare: None,
        formatter: None,
    };
    create_instance(
        ctx,
        state,
        IntlCallable::CollatorConstructor,
        IntlSlot::Collator(slot),
        this_value,
    )
}

fn resolved(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::Collator(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let locale = slot.locale.clone();
    let usage = slot.usage.clone();
    let sensitivity = slot.sensitivity.clone();
    let case_first = slot.case_first.clone();
    let collation = slot.collation.clone();
    let ignore_punctuation = slot.ignore_punctuation;
    let numeric = slot.numeric;
    let locale = intern(ctx, state, locale);
    let usage = intern(ctx, state, usage);
    let sensitivity = intern(ctx, state, sensitivity);
    let case_first = intern(ctx, state, case_first);
    let collation = intern(ctx, state, collation);
    resolved_object(
        ctx,
        state,
        &[
            ("locale", locale),
            ("usage", usage),
            ("sensitivity", sensitivity),
            ("ignorePunctuation", value::encode_bool(ignore_punctuation)),
            ("collation", collation),
            ("numeric", value::encode_bool(numeric)),
            ("caseFirst", case_first),
        ],
    )
}

fn compare_get(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::Collator(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    if let Some(bound) = slot.bound_compare {
        return bound;
    }
    let Some(bound) = state.native_callable(NativeCallableKind::Intl(
        IntlCallable::CollatorCompare(handle),
    )) else {
        return fail_dispatch(ctx);
    };
    if let Some(IntlSlot::Collator(slot)) = state.intl.slots.get_mut(&handle) {
        slot.bound_compare = Some(bound);
    }
    bound
}

fn compare(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    args: &[i64],
) -> i64 {
    let left = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let right = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let left = match to_string_coerced(ctx, state, left) {
        Ok(text) => text,
        Err(exception) => return exception,
    };
    let right = match to_string_coerced(ctx, state, right) {
        Ok(text) => text,
        Err(exception) => return exception,
    };
    if let Err(exception) = ensure_formatter(ctx, state, handle) {
        return exception;
    }
    let Some(IntlSlot::Collator(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let Some(formatter) = slot.formatter.as_ref() else {
        return fail_dispatch(ctx);
    };
    value::encode_f64(f64::from(formatter.compare(&left, &right)))
}

fn ensure_formatter(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
) -> Result<(), i64> {
    let Some(IntlSlot::Collator(slot)) = state.intl.slots.get(&handle) else {
        return Err(incompatible(ctx, state));
    };
    if slot.formatter.is_some() {
        return Ok(());
    }
    let sensitivity = match slot.sensitivity.as_str() {
        "base" => CollatorSensitivity::Base,
        "accent" => CollatorSensitivity::Accent,
        "case" => CollatorSensitivity::Case,
        _ => CollatorSensitivity::Variant,
    };
    let locale = slot.locale.clone();
    let numeric = slot.numeric;
    let case_first = slot.case_first.clone();
    let ignore = slot.ignore_punctuation;
    let collation = if slot.usage == "search" {
        "search".to_owned()
    } else {
        slot.collation.clone()
    };
    let formatter = OwnedCollator::try_new(
        &locale,
        sensitivity,
        numeric,
        Some(case_first.as_str()),
        ignore,
        Some(collation.as_str()),
    )
    .map_err(|error| crate::dispatch::runtime::range_error(ctx, state, &error))?;
    if let Some(IntlSlot::Collator(slot)) = state.intl.slots.get_mut(&handle) {
        slot.formatter = Some(formatter);
    }
    Ok(())
}
