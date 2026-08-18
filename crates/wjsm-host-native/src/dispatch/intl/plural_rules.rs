//! `Intl.PluralRules`。

use wjsm_builtins::intl::resolve_locale;
use wjsm_intl_data::OwnedPluralRules;
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::common::{create_instance, intern, resolved_object, slot_handle, throw_intl};
use super::js::{
    canonicalize_locales, get_number_option, get_option_string, get_options_object,
    supported_locales_of,
};
use super::slots::{IntlSlot, PluralRulesSlot};
use super::{IntlCallable, incompatible};
use crate::NativeAgentState;
use crate::dispatch::runtime::{fail_dispatch, range_error, to_number_coerced, type_error};

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: IntlCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    match callable {
        IntlCallable::PluralRulesConstructor => construct(ctx, state, receiver, args),
        IntlCallable::PluralRulesSupportedLocalesOf => supported_locales_of(ctx, state, args),
        IntlCallable::PluralRulesResolvedOptions => resolved(ctx, state, receiver),
        IntlCallable::PluralRulesSelect => select(ctx, state, receiver, args, false),
        IntlCallable::PluralRulesSelectRange => select(ctx, state, receiver, args, true),
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
        &["cardinal", "ordinal"],
        Some("cardinal"),
    ) {
        Ok(value) => value.unwrap_or_else(|| "cardinal".into()),
        Err(exception) => return exception,
    };
    let notation = match get_option_string(
        ctx,
        state,
        options,
        "notation",
        &["standard", "compact", "scientific", "engineering"],
        Some("standard"),
    ) {
        Ok(value) => value.unwrap_or_else(|| "standard".into()),
        Err(exception) => return exception,
    };
    let digits = match read_plural_digits(ctx, state, options) {
        Ok(digits) => digits,
        Err(exception) => return exception,
    };
    let resolved = match resolve_locale(&requested, &[], &Default::default()) {
        Ok(resolved) => resolved,
        Err(error) => return throw_intl(ctx, state, error),
    };
    create_instance(
        ctx,
        state,
        IntlCallable::PluralRulesConstructor,
        IntlSlot::PluralRules(PluralRulesSlot {
            locale: resolved.locale,
            type_name,
            notation,
            minimum_integer_digits: digits.0,
            minimum_fraction_digits: digits.1,
            maximum_fraction_digits: digits.2,
            minimum_significant_digits: digits.3,
            maximum_significant_digits: digits.4,
            rounding_increment: digits.5,
            rounding_mode: digits.6,
            rounding_priority: digits.7,
            trailing_zero_display: digits.8,
            formatter: None,
        }),
        this_value,
    )
}

fn resolved(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::PluralRules(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let locale = slot.locale.clone();
    let type_name = slot.type_name.clone();
    let notation = slot.notation.clone();
    let min_int = slot.minimum_integer_digits;
    let min_frac = slot.minimum_fraction_digits;
    let max_frac = slot.maximum_fraction_digits;
    let rounding_mode = slot.rounding_mode.clone();
    let rounding_increment = slot.rounding_increment;
    let rounding_priority = slot.rounding_priority.clone();
    let trailing_zero_display = slot.trailing_zero_display.clone();
    let min_sig = slot.minimum_significant_digits;
    let max_sig = slot.maximum_significant_digits;
    if let Err(exception) = ensure(ctx, state, handle) {
        return exception;
    }
    let Some(IntlSlot::PluralRules(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let categories = slot
        .formatter
        .as_ref()
        .map(OwnedPluralRules::categories)
        .unwrap_or_else(|| vec!["other".into()]);
    let locale = intern(ctx, state, locale);
    let type_name = intern(ctx, state, type_name);
    let notation = intern(ctx, state, notation);
    let plural_categories = super::js::string_array(ctx, state, &categories);
    let mut fields = vec![
        ("locale", locale),
        ("type", type_name),
        ("notation", notation),
        ("minimumIntegerDigits", value::encode_f64(min_int as f64)),
        ("minimumFractionDigits", value::encode_f64(min_frac as f64)),
        ("maximumFractionDigits", value::encode_f64(max_frac as f64)),
    ];
    if let Some(min_sig) = min_sig {
        fields.push((
            "minimumSignificantDigits",
            value::encode_f64(min_sig as f64),
        ));
        if let Some(max_sig) = max_sig {
            fields.push((
                "maximumSignificantDigits",
                value::encode_f64(max_sig as f64),
            ));
        }
    }
    fields.push(("pluralCategories", plural_categories));
    fields.push((
        "roundingIncrement",
        value::encode_f64(rounding_increment as f64),
    ));
    fields.push(("roundingMode", intern(ctx, state, rounding_mode)));
    fields.push(("roundingPriority", intern(ctx, state, rounding_priority)));
    fields.push((
        "trailingZeroDisplay",
        intern(ctx, state, trailing_zero_display),
    ));
    resolved_object(ctx, state, &fields)
}

fn select(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
    range: bool,
) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    if !matches!(
        state.intl.slots.get(&handle),
        Some(IntlSlot::PluralRules(_))
    ) {
        return incompatible(ctx, state);
    }
    let start = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let end = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if range && (value::is_undefined(start) || value::is_undefined(end)) {
        return type_error(ctx, state, "selectRange requires two arguments");
    }
    let start_number = match to_number_coerced(ctx, state, start) {
        Ok(number) => number,
        Err(exception) => return exception,
    };
    let end_number = if range {
        match to_number_coerced(ctx, state, end) {
            Ok(number) => number,
            Err(exception) => return exception,
        }
    } else {
        0.0
    };
    if range && (start_number.is_nan() || end_number.is_nan()) {
        return range_error(ctx, state, "selectRange arguments must be finite numbers");
    }
    if let Err(exception) = ensure(ctx, state, handle) {
        return exception;
    }
    let Some(IntlSlot::PluralRules(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let notation = slot.notation.clone();
    let category = slot.formatter.as_ref().and_then(|formatter| {
        if range {
            formatter.select_range(start_number, end_number).ok()
        } else {
            formatter.select_with_notation(start_number, &notation).ok()
        }
    });
    intern(ctx, state, category.unwrap_or_else(|| "other".into()))
}

fn read_plural_digits(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
) -> Result<
    (
        u32,
        u32,
        u32,
        Option<u32>,
        Option<u32>,
        u32,
        String,
        String,
        String,
    ),
    i64,
> {
    let min_int = get_number_option(
        ctx,
        state,
        options,
        "minimumIntegerDigits",
        1.0,
        21.0,
        Some(1.0),
    )?
    .unwrap_or(1.0) as u32;
    let min_frac = get_number_option(
        ctx,
        state,
        options,
        "minimumFractionDigits",
        0.0,
        100.0,
        None,
    )?;
    let max_frac = get_number_option(
        ctx,
        state,
        options,
        "maximumFractionDigits",
        0.0,
        100.0,
        None,
    )?;
    let (min_frac, max_frac) = match (min_frac, max_frac) {
        (None, None) => (0, 3),
        (Some(min), None) => (min as u32, 3u32.max(min as u32)),
        (None, Some(max)) => (0u32.min(max as u32), max as u32),
        (Some(min), Some(max)) if (max as u32) < (min as u32) => {
            return Err(range_error(
                ctx,
                state,
                "maximumFractionDigits < minimumFractionDigits",
            ));
        }
        (Some(min), Some(max)) => (min as u32, max as u32),
    };
    let min_sig = get_number_option(
        ctx,
        state,
        options,
        "minimumSignificantDigits",
        1.0,
        21.0,
        None,
    )?
    .map(|value| value as u32);
    let max_sig = get_number_option(
        ctx,
        state,
        options,
        "maximumSignificantDigits",
        1.0,
        21.0,
        None,
    )?
    .map(|value| value as u32);
    let (min_sig, max_sig) = match (min_sig, max_sig) {
        (None, None) => (None, None),
        (Some(min), None) => (Some(min), Some(21)),
        (None, Some(max)) => (Some(1), Some(max)),
        (Some(min), Some(max)) if max < min => {
            return Err(range_error(
                ctx,
                state,
                "maximumSignificantDigits < minimumSignificantDigits",
            ));
        }
        (Some(min), Some(max)) => (Some(min), Some(max)),
    };
    let rounding_increment = get_number_option(
        ctx,
        state,
        options,
        "roundingIncrement",
        1.0,
        5000.0,
        Some(1.0),
    )?
    .unwrap_or(1.0) as u32;
    let rounding_mode = get_option_string(
        ctx,
        state,
        options,
        "roundingMode",
        &[
            "ceil",
            "floor",
            "expand",
            "trunc",
            "halfCeil",
            "halfFloor",
            "halfExpand",
            "halfTrunc",
            "halfEven",
        ],
        Some("halfExpand"),
    )?
    .unwrap_or_else(|| "halfExpand".into());
    let rounding_priority = get_option_string(
        ctx,
        state,
        options,
        "roundingPriority",
        &["auto", "morePrecision", "lessPrecision"],
        Some("auto"),
    )?
    .unwrap_or_else(|| "auto".into());
    let trailing_zero_display = get_option_string(
        ctx,
        state,
        options,
        "trailingZeroDisplay",
        &["auto", "stripIfInteger"],
        Some("auto"),
    )?
    .unwrap_or_else(|| "auto".into());
    Ok((
        min_int,
        min_frac,
        max_frac,
        min_sig,
        max_sig,
        rounding_increment,
        rounding_mode,
        rounding_priority,
        trailing_zero_display,
    ))
}

fn ensure(ctx: &mut NativeVmContext, state: &mut NativeAgentState, handle: u32) -> Result<(), i64> {
    let Some(IntlSlot::PluralRules(slot)) = state.intl.slots.get(&handle) else {
        return Err(incompatible(ctx, state));
    };
    if slot.formatter.is_some() {
        return Ok(());
    }
    let locale = slot.locale.clone();
    let ordinal = slot.type_name == "ordinal";
    let formatter = OwnedPluralRules::try_new(&locale, ordinal)
        .map_err(|error| range_error(ctx, state, &error))?;
    if let Some(IntlSlot::PluralRules(slot)) = state.intl.slots.get_mut(&handle) {
        slot.formatter = Some(formatter);
    }
    Ok(())
}
