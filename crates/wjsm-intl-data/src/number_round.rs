//! NumberFormat 舍入与最小位数。

use fixed_decimal::{RoundingIncrement, SignedRoundingMode, UnsignedRoundingMode};
use icu::decimal::input::Decimal;

use crate::number::NumberFormatSpec;

pub(crate) fn apply_rounding(decimal: &mut Decimal, spec: &NumberFormatSpec) -> bool {
    let mode = rounding_mode(spec);
    let has_sig = spec.maximum_significant_digits.is_some();
    let use_sig = match spec.rounding_priority.as_str() {
        "morePrecision" if has_sig => prefer_significant(decimal, spec, mode, true),
        "lessPrecision" if has_sig => prefer_significant(decimal, spec, mode, false),
        _ if has_sig => {
            round_significant(decimal, spec, mode);
            true
        }
        _ => {
            round_fraction(decimal, spec, mode);
            false
        }
    };
    apply_minimums(decimal, spec, use_sig);
    use_sig
}

fn prefer_significant(
    decimal: &mut Decimal,
    spec: &NumberFormatSpec,
    mode: SignedRoundingMode,
    more: bool,
) -> bool {
    let sig_pos = significant_position(decimal, spec);
    let frac_pos = increment_at(spec.maximum_fraction_digits, spec.rounding_increment).0;
    let use_sig = if more {
        sig_pos <= frac_pos
    } else {
        sig_pos > frac_pos
    };
    if use_sig {
        round_significant(decimal, spec, mode);
    } else {
        round_fraction(decimal, spec, mode);
    }
    use_sig
}

fn rounding_mode(spec: &NumberFormatSpec) -> SignedRoundingMode {
    match spec.rounding_mode.as_str() {
        "ceil" => SignedRoundingMode::Ceil,
        "floor" => SignedRoundingMode::Floor,
        "expand" => SignedRoundingMode::Unsigned(UnsignedRoundingMode::Expand),
        "trunc" => SignedRoundingMode::Unsigned(UnsignedRoundingMode::Trunc),
        "halfCeil" => SignedRoundingMode::HalfCeil,
        "halfFloor" => SignedRoundingMode::HalfFloor,
        "halfTrunc" => SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfTrunc),
        "halfEven" => SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfEven),
        _ => SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
    }
}

fn round_significant(decimal: &mut Decimal, spec: &NumberFormatSpec, mode: SignedRoundingMode) {
    if spec.maximum_significant_digits.is_none() || decimal.is_zero() {
        return;
    }
    *decimal = decimal.clone().rounded_with_mode_and_increment(
        significant_position(decimal, spec),
        mode,
        RoundingIncrement::MultiplesOf1,
    );
}

fn significant_position(decimal: &Decimal, spec: &NumberFormatSpec) -> i16 {
    let max_sig = spec.maximum_significant_digits.unwrap_or(21);
    let start = if decimal.is_zero() {
        0
    } else {
        decimal.nonzero_magnitude_start()
    };
    start - max_sig as i16 + 1
}

fn round_fraction(decimal: &mut Decimal, spec: &NumberFormatSpec, mode: SignedRoundingMode) {
    let (position, increment) = increment_at(spec.maximum_fraction_digits, spec.rounding_increment);
    *decimal = decimal
        .clone()
        .rounded_with_mode_and_increment(position, mode, increment);
}

fn apply_minimums(decimal: &mut Decimal, spec: &NumberFormatSpec, use_sig: bool) {
    if use_sig {
        if let Some(min_sig) = spec.minimum_significant_digits {
            let start = if decimal.is_zero() {
                0
            } else {
                decimal.nonzero_magnitude_start()
            };
            decimal.pad_end(start - min_sig as i16 + 1);
        }
    } else {
        decimal.trim_end();
        decimal.pad_end(-(spec.minimum_fraction_digits as i16));
    }
    if spec.minimum_integer_digits > 1 {
        decimal.pad_start(spec.minimum_integer_digits as i16 - 1);
    }
}

fn increment_at(max_frac: u32, increment: u32) -> (i16, RoundingIncrement) {
    let (shift, kind) = match increment {
        1 => (0, RoundingIncrement::MultiplesOf1),
        2 => (0, RoundingIncrement::MultiplesOf2),
        5 => (0, RoundingIncrement::MultiplesOf5),
        10 => (1, RoundingIncrement::MultiplesOf1),
        20 => (1, RoundingIncrement::MultiplesOf2),
        25 => (0, RoundingIncrement::MultiplesOf25),
        50 => (1, RoundingIncrement::MultiplesOf5),
        100 => (2, RoundingIncrement::MultiplesOf1),
        200 => (2, RoundingIncrement::MultiplesOf2),
        250 => (1, RoundingIncrement::MultiplesOf25),
        500 => (2, RoundingIncrement::MultiplesOf5),
        1000 => (3, RoundingIncrement::MultiplesOf1),
        2000 => (3, RoundingIncrement::MultiplesOf2),
        2500 => (2, RoundingIncrement::MultiplesOf25),
        5000 => (3, RoundingIncrement::MultiplesOf5),
        _ => (0, RoundingIncrement::MultiplesOf1),
    };
    (-(max_frac as i16) + shift, kind)
}
