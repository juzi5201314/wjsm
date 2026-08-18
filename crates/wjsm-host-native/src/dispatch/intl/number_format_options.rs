//! `Intl.NumberFormat` 选项读取。

use std::collections::BTreeMap;

use wjsm_builtins::intl::resolve_locale_filtered;
use wjsm_intl_data::{
    available_numbering_systems, currency_digits, is_well_formed_unit_identifier,
};
use wjsm_native_abi::NativeVmContext;

use super::common::throw_intl;
use super::js::{
    get_named, get_number_option, get_option_string, require_unicode_type, to_boolean,
};
use super::slots::NumberFormatSlot;
use crate::NativeAgentState;
use crate::dispatch::runtime::{range_error, to_string_coerced, type_error};
use wjsm_ir::value;

pub(super) fn read_options(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    requested: &[String],
    options: i64,
) -> Result<NumberFormatSlot, i64> {
    let _ = get_option_string(
        ctx,
        state,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    )?;
    let numbering_system = require_unicode_type(ctx, state, options, "numberingSystem")?;
    let style = get_option_string(
        ctx,
        state,
        options,
        "style",
        &["decimal", "percent", "currency", "unit"],
        Some("decimal"),
    )?
    .unwrap_or_else(|| "decimal".into());
    let currency = get_option_string(ctx, state, options, "currency", &[], None)?;
    if style == "currency" && currency.is_none() {
        return Err(type_error(ctx, state, "currency is required"));
    }
    if let Some(code) = &currency
        && !is_currency_code(code)
    {
        return Err(range_error(ctx, state, "invalid currency code"));
    }
    let currency_display = get_option_string(
        ctx,
        state,
        options,
        "currencyDisplay",
        &["code", "symbol", "narrowSymbol", "name"],
        Some("symbol"),
    )?
    .unwrap_or_else(|| "symbol".into());
    let currency_sign = get_option_string(
        ctx,
        state,
        options,
        "currencySign",
        &["standard", "accounting"],
        Some("standard"),
    )?
    .unwrap_or_else(|| "standard".into());
    let unit = get_option_string(ctx, state, options, "unit", &[], None)?;
    if style == "unit" && unit.is_none() {
        return Err(type_error(ctx, state, "unit is required"));
    }
    if let Some(unit) = &unit
        && !is_well_formed_unit_identifier(unit)
    {
        return Err(range_error(ctx, state, "invalid unit"));
    }
    let unit_display = get_option_string(
        ctx,
        state,
        options,
        "unitDisplay",
        &["short", "narrow", "long"],
        Some("short"),
    )?
    .unwrap_or_else(|| "short".into());
    let notation = get_option_string(
        ctx,
        state,
        options,
        "notation",
        &["standard", "scientific", "engineering", "compact"],
        Some("standard"),
    )?
    .unwrap_or_else(|| "standard".into());
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
    let (min_frac, max_frac) =
        fraction_digits(ctx, state, options, &style, &notation, currency.as_deref())?;
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
    if !matches!(
        rounding_increment,
        1 | 2 | 5 | 10 | 20 | 25 | 50 | 100 | 200 | 250 | 500 | 1000 | 2000 | 2500 | 5000
    ) {
        return Err(range_error(ctx, state, "roundingIncrement is out of range"));
    }
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
    if rounding_increment != 1 {
        if min_sig.is_some() || rounding_priority != "auto" {
            return Err(type_error(
                ctx,
                state,
                "roundingIncrement cannot combine with significant digits",
            ));
        }
        if min_frac != max_frac {
            return Err(range_error(
                ctx,
                state,
                "roundingIncrement requires equal fraction digits",
            ));
        }
    }
    let compact_display = get_option_string(
        ctx,
        state,
        options,
        "compactDisplay",
        &["short", "long"],
        Some("short"),
    )?
    .unwrap_or_else(|| "short".into());
    let (mut use_grouping, grouping_omitted) = grouping_option(ctx, state, options)?;
    if notation == "compact" && grouping_omitted {
        use_grouping = "min2".into();
    }
    let sign_display = get_option_string(
        ctx,
        state,
        options,
        "signDisplay",
        &["auto", "never", "always", "exceptZero", "negative"],
        Some("auto"),
    )?
    .unwrap_or_else(|| "auto".into());
    let mut ext = BTreeMap::new();
    if let Some(system) = numbering_system
        .as_deref()
        .filter(|system| available_numbering_systems().contains(system))
    {
        ext.insert("nu".into(), system.to_owned());
    }
    let resolved = resolve_locale_filtered(requested, &["nu"], &ext, |key, value, _| {
        key == "nu" && available_numbering_systems().contains(&value)
    })
    .map_err(|error| throw_intl(ctx, state, error))?;
    Ok(NumberFormatSlot {
        locale: resolved.locale,
        numbering_system: resolved
            .extensions
            .get("nu")
            .cloned()
            .unwrap_or_else(|| "latn".into()),
        style: style.clone(),
        currency: if style == "currency" {
            currency.map(|code| code.to_ascii_uppercase())
        } else {
            None
        },
        currency_display,
        currency_sign,
        unit,
        unit_display,
        notation,
        compact_display,
        sign_display,
        use_grouping,
        minimum_integer_digits: min_int,
        minimum_fraction_digits: min_frac,
        maximum_fraction_digits: max_frac,
        minimum_significant_digits: min_sig,
        maximum_significant_digits: max_sig,
        rounding_mode,
        rounding_increment,
        rounding_priority,
        trailing_zero_display,
        bound_format: None,
        formatter: None,
    })
}

fn grouping_option(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
) -> Result<(String, bool), i64> {
    let value = get_named(ctx, state, options, "useGrouping")?;
    if value::is_undefined(value) {
        return Ok(("auto".into(), true));
    }
    if value::is_bool(value) && value::decode_bool(value) {
        return Ok(("always".into(), false));
    }
    if !to_boolean(state, value) {
        return Ok(("false".into(), false));
    }
    let text = to_string_coerced(ctx, state, value)?;
    if matches!(text.as_str(), "min2" | "auto" | "always") {
        return Ok((text, false));
    }
    Err(range_error(ctx, state, "useGrouping is out of range"))
}

fn fraction_digits(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
    style: &str,
    notation: &str,
    currency: Option<&str>,
) -> Result<(u32, u32), i64> {
    let (default_min, default_max) = digit_defaults(style, notation, currency);
    let min = get_number_option(
        ctx,
        state,
        options,
        "minimumFractionDigits",
        0.0,
        100.0,
        None,
    )?;
    let max = get_number_option(
        ctx,
        state,
        options,
        "maximumFractionDigits",
        0.0,
        100.0,
        None,
    )?;
    match (min, max) {
        (None, None) => Ok((default_min, default_max)),
        (Some(min), None) => {
            let min = min as u32;
            Ok((min, default_max.max(min)))
        }
        (None, Some(max)) => {
            let max = max as u32;
            Ok((default_min.min(max), max))
        }
        (Some(min), Some(max)) => {
            let min = min as u32;
            let max = max as u32;
            if max < min {
                return Err(range_error(
                    ctx,
                    state,
                    "maximumFractionDigits < minimumFractionDigits",
                ));
            }
            Ok((min, max))
        }
    }
}

fn digit_defaults(style: &str, notation: &str, currency: Option<&str>) -> (u32, u32) {
    if style == "currency" && notation == "standard" {
        let digits = currency_digits(currency.unwrap_or("USD"));
        (digits, digits)
    } else if style == "percent" || (style == "currency" && notation == "compact") {
        (0, 0)
    } else {
        (0, 3)
    }
}

fn is_currency_code(code: &str) -> bool {
    code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_alphabetic())
}
