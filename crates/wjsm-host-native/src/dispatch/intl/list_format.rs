//! `Intl.ListFormat`。

use wjsm_builtins::intl::resolve_locale;
use wjsm_intl_data::OwnedListFormatter;
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::common::{create_instance, intern, parts_array, slot_handle, throw_intl};
use super::js::{
    canonicalize_locales, get_option_string, get_options_object, supported_locales_of,
};
use super::slots::{IntlSlot, ListFormatSlot};
use super::{IntlCallable, incompatible};
use crate::NativeAgentState;
use crate::dispatch::runtime::{
    fail_dispatch, is_truthy, iterator_close, iterator_from, iterator_next_result, range_error,
    to_string_coerced, type_error,
};

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: IntlCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    match callable {
        IntlCallable::ListFormatConstructor => construct(ctx, state, receiver, args),
        IntlCallable::ListFormatSupportedLocalesOf => supported_locales_of(ctx, state, args),
        IntlCallable::ListFormatResolvedOptions => resolved(ctx, state, receiver),
        IntlCallable::ListFormatFormat => format_list(ctx, state, receiver, args, false),
        IntlCallable::ListFormatFormatToParts => format_list(ctx, state, receiver, args, true),
        _ => fail_dispatch(ctx),
    }
}

fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    if let Err(exception) = super::common::require_new(ctx, state) {
        return exception;
    }
    let requested = match canonicalize_locales(
        ctx,
        state,
        args.first()
            .copied()
            .unwrap_or_else(value::encode_undefined),
    ) {
        Ok(requested) => requested,
        Err(exception) => return exception,
    };
    let options = match get_options_object(
        ctx,
        state,
        args.get(1).copied().unwrap_or_else(value::encode_undefined),
    ) {
        Ok(options) => options,
        Err(exception) => return exception,
    };
    let _ = match get_option_string(
        ctx,
        state,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    ) {
        Ok(_) => {}
        Err(exception) => return exception,
    };
    let type_name = match get_option_string(
        ctx,
        state,
        options,
        "type",
        &["conjunction", "disjunction", "unit"],
        Some("conjunction"),
    ) {
        Ok(value) => value.unwrap_or_else(|| "conjunction".into()),
        Err(exception) => return exception,
    };
    let style = match get_option_string(
        ctx,
        state,
        options,
        "style",
        &["long", "short", "narrow"],
        Some("long"),
    ) {
        Ok(value) => value.unwrap_or_else(|| "long".into()),
        Err(exception) => return exception,
    };
    let resolved = match resolve_locale(&requested, &[], &Default::default()) {
        Ok(resolved) => resolved,
        Err(error) => return throw_intl(ctx, state, error),
    };
    create_instance(
        ctx,
        state,
        IntlCallable::ListFormatConstructor,
        IntlSlot::ListFormat(ListFormatSlot {
            locale: resolved.locale,
            type_name,
            style,
            formatter: None,
        }),
        this_value,
    )
}

fn resolved(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::ListFormat(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let pairs = [
        ("locale", slot.locale.clone()),
        ("type", slot.type_name.clone()),
        ("style", slot.style.clone()),
    ];
    super::common::resolved_strings(ctx, state, &pairs)
}

fn format_list(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
    parts: bool,
) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    if !matches!(state.intl.slots.get(&handle), Some(IntlSlot::ListFormat(_))) {
        return incompatible(ctx, state);
    }
    let list = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_undefined(list) {
        return if parts {
            state
                .allocate_array_values(&[])
                .unwrap_or_else(|_| fail_dispatch(ctx))
        } else {
            intern(ctx, state, "")
        };
    }
    let items = match collect_strings(ctx, state, list) {
        Ok(items) => items,
        Err(exception) => return exception,
    };
    if let Err(exception) = ensure(ctx, state, handle) {
        return exception;
    }
    let Some(IntlSlot::ListFormat(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    let formatter = slot.formatter.as_ref();
    if parts {
        let parts = formatter
            .map(|formatter| formatter.format_parts(&refs).unwrap_or_default())
            .unwrap_or_default();
        return parts_array(ctx, state, parts);
    }
    let rendered = formatter
        .map(|formatter| formatter.format(&refs))
        .unwrap_or_else(|| items.join(", "));
    intern(ctx, state, rendered)
}

fn collect_strings(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    list: i64,
) -> Result<Vec<String>, i64> {
    if value::is_string(list) {
        let text = to_string_coerced(ctx, state, list)?;
        return Ok(text.chars().map(|ch| ch.to_string()).collect());
    }
    let iterator = iterator_from(ctx, state, &[list]);
    if value::is_exception(iterator) {
        return Err(iterator);
    }
    let mut items = Vec::new();
    loop {
        let result = iterator_next_result(ctx, state, value::decode_handle(iterator));
        if value::is_exception(result) {
            return Err(result);
        }
        let done = super::js::get_named(ctx, state, result, "done")?;
        if is_truthy(state, done) {
            break;
        }
        let item = match super::js::get_named(ctx, state, result, "value") {
            Ok(item) => item,
            Err(exception) => return Err(iterator_close(ctx, state, &[iterator, exception])),
        };
        if !value::is_string(item) {
            let exception = type_error(ctx, state, "list item must be a string");
            return Err(iterator_close(ctx, state, &[iterator, exception]));
        }
        items.push(to_string_coerced(ctx, state, item)?);
    }
    Ok(items)
}

fn ensure(ctx: &mut NativeVmContext, state: &mut NativeAgentState, handle: u32) -> Result<(), i64> {
    let Some(IntlSlot::ListFormat(slot)) = state.intl.slots.get(&handle) else {
        return Err(incompatible(ctx, state));
    };
    if slot.formatter.is_some() {
        return Ok(());
    }
    let locale = slot.locale.clone();
    let type_name = slot.type_name.clone();
    let style = slot.style.clone();
    let formatter = OwnedListFormatter::try_new(&locale, &type_name, &style)
        .map_err(|error| range_error(ctx, state, &error))?;
    if let Some(IntlSlot::ListFormat(slot)) = state.intl.slots.get_mut(&handle) {
        slot.formatter = Some(formatter);
    }
    Ok(())
}
