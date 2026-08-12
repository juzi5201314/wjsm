use std::cmp::Ordering;

use num_bigint::{BigUint, Sign};
use num_traits::{One, ToPrimitive, Zero};

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{
    PrimitiveHint, fail_dispatch, is_truthy, range_error, to_int32, to_number, to_number_coerced,
    to_primitive, to_string_coerced, type_error,
};
use crate::NativeAgentState;

pub(super) fn dispatch_primitive(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::NumberConstructor => number_constructor(ctx, state, args),
        Builtin::NumberIsNaN => value::encode_bool(
            args.first()
                .is_some_and(|value| value::is_f64(*value) && value::decode_f64(*value).is_nan()),
        ),
        Builtin::NumberIsFinite => {
            value::encode_bool(args.first().is_some_and(|value| {
                value::is_f64(*value) && value::decode_f64(*value).is_finite()
            }))
        }
        Builtin::GlobalIsNaN | Builtin::GlobalIsFinite => {
            let argument = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            match to_number_coerced(ctx, state, argument) {
                Ok(number) => value::encode_bool(if builtin == Builtin::GlobalIsNaN {
                    number.is_nan()
                } else {
                    number.is_finite()
                }),
                Err(exception) => exception,
            }
        }
        Builtin::NumberIsInteger => value::encode_bool(args.first().is_some_and(|value| {
            value::is_f64(*value) && {
                let number = value::decode_f64(*value);
                number.is_finite() && number.fract() == 0.0
            }
        })),
        Builtin::NumberIsSafeInteger => value::encode_bool(args.first().is_some_and(|value| {
            value::is_f64(*value) && {
                let number = value::decode_f64(*value);
                number.is_finite()
                    && number.fract() == 0.0
                    && number.abs() <= 9_007_199_254_740_991.0
            }
        })),
        Builtin::NumberParseInt => parse_int(ctx, state, args),
        Builtin::NumberParseFloat => parse_float(ctx, state, args),
        Builtin::NumberProtoValueOf => args
            .first()
            .filter(|value| value::is_f64(**value))
            .copied()
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::NumberProtoToString => number_to_string(ctx, state, args),
        Builtin::NumberProtoToFixed => number_to_fixed(ctx, state, args),
        Builtin::NumberProtoToExponential => number_to_exponential(ctx, state, args),
        Builtin::NumberProtoToPrecision => number_to_precision(ctx, state, args),
        Builtin::BooleanConstructor => value::encode_bool(
            args.first()
                .is_some_and(|argument| is_truthy(state, *argument)),
        ),
        Builtin::BooleanProtoValueOf => args
            .first()
            .filter(|value| value::is_bool(**value))
            .copied()
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::BooleanProtoToString => {
            let Some(boolean) = args.first().filter(|value| value::is_bool(**value)) else {
                return Some(fail_dispatch(ctx));
            };
            state
                .intern_text(value::decode_bool(*boolean).to_string(), value::TAG_STRING)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        _ => return None,
    })
}

fn number_constructor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(argument) = args.first().copied() else {
        return value::encode_f64(0.0);
    };
    let primitive = match to_primitive(ctx, state, argument, PrimitiveHint::Number) {
        Ok(primitive) => primitive,
        Err(exception) => return exception,
    };
    if value::is_bigint(primitive) {
        let Some(integer) = super::bigint::read(state, primitive) else {
            return fail_dispatch(ctx);
        };
        let number = integer.to_f64().unwrap_or_else(|| {
            if integer.sign() == Sign::Minus {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }
        });
        return value::encode_f64(number);
    }
    if value::is_symbol(primitive) {
        return type_error(ctx, state, "Cannot convert a Symbol value to a number");
    }
    to_number(state, primitive)
        .map(value::encode_f64)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn number_to_string(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(encoded) = args.first().filter(|value| value::is_f64(**value)) else {
        return fail_dispatch(ctx);
    };
    let number = value::decode_f64(*encoded);
    let radix = args
        .get(1)
        .and_then(|radix| to_number(state, *radix))
        .map(to_int32)
        .unwrap_or(10);
    if !(2..=36).contains(&radix) {
        return range_error(
            ctx,
            state,
            "toString() radix argument must be between 2 and 36",
        );
    }
    let text = if radix == 10 || !number.is_finite() {
        render_decimal(number)
    } else {
        format_radix(number, radix as u32)
    };
    state
        .intern_text(text, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn number_to_fixed(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(number) = this_number_value(state, args) else {
        return type_error(
            ctx,
            state,
            "Number.prototype.toFixed called on incompatible receiver",
        );
    };
    let digits = match decimal_digits(ctx, state, args.get(1).copied(), 0, 0, 100) {
        Ok(digits) => digits,
        Err(exception) => return exception,
    };
    let text = if !number.is_finite() || number.abs() >= 1e21 {
        render_decimal(number)
    } else {
        format_fixed(number, digits)
    };
    intern_number_text(ctx, state, text)
}

fn number_to_exponential(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(number) = this_number_value(state, args) else {
        return type_error(
            ctx,
            state,
            "Number.prototype.toExponential called on incompatible receiver",
        );
    };
    let digits = if args
        .get(1)
        .is_none_or(|digits| value::is_undefined(*digits))
    {
        None
    } else {
        match decimal_digits(ctx, state, args.get(1).copied(), 0, 0, 100) {
            Ok(digits) => Some(digits),
            Err(exception) => return exception,
        }
    };
    let text = if number.is_finite() {
        format_exponential(number, digits)
    } else {
        render_decimal(number)
    };
    intern_number_text(ctx, state, text)
}

fn number_to_precision(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(number) = this_number_value(state, args) else {
        return type_error(
            ctx,
            state,
            "Number.prototype.toPrecision called on incompatible receiver",
        );
    };
    if args
        .get(1)
        .is_none_or(|precision| value::is_undefined(*precision))
    {
        return intern_number_text(ctx, state, render_decimal(number));
    }
    let precision = match decimal_digits(ctx, state, args.get(1).copied(), 1, 1, 100) {
        Ok(precision) => precision,
        Err(exception) => return exception,
    };
    let text = if number.is_finite() {
        format_precision(number, precision)
    } else {
        render_decimal(number)
    };
    intern_number_text(ctx, state, text)
}

fn this_number_value(state: &NativeAgentState, args: &[i64]) -> Option<f64> {
    let receiver = args.first().copied()?;
    if value::is_f64(receiver) {
        return Some(value::decode_f64(receiver));
    }
    value::is_js_object(receiver)
        .then(|| value::decode_handle(receiver))
        .and_then(|handle| state.boxed_primitives.get(&handle))
        .copied()
        .filter(|primitive| value::is_f64(*primitive))
        .map(value::decode_f64)
}

fn decimal_digits(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    argument: Option<i64>,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize, i64> {
    let Some(argument) = argument else {
        return Ok(default);
    };
    let number = to_number_coerced(ctx, state, argument)?;
    let integer = if number.is_nan() || number == 0.0 {
        0.0
    } else {
        number.trunc()
    };
    if !integer.is_finite() || integer < minimum as f64 || integer > maximum as f64 {
        return Err(range_error(
            ctx,
            state,
            "number of digits is outside the supported range",
        ));
    }
    Ok(integer as usize)
}

fn intern_number_text(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    text: String,
) -> i64 {
    state
        .intern_text(text, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn render_decimal(number: f64) -> String {
    if number.is_nan() {
        return "NaN".into();
    }
    if number == f64::INFINITY {
        return "Infinity".into();
    }
    if number == f64::NEG_INFINITY {
        return "-Infinity".into();
    }
    if number == 0.0 {
        return "0".into();
    }
    let negative = number < 0.0;
    let (digits, exponent) = shortest_decimal_components(number.abs());
    let decimal_position = exponent + 1;
    let mut text = if (1..=21).contains(&decimal_position) {
        let position = decimal_position as usize;
        if position >= digits.len() {
            let mut text = digits;
            text.push_str(&"0".repeat(position - text.len()));
            text
        } else {
            let mut text = digits;
            text.insert(position, '.');
            text
        }
    } else if (-5..=0).contains(&decimal_position) {
        format!("0.{}{}", "0".repeat((-decimal_position) as usize), digits)
    } else {
        scientific_notation(&digits, exponent)
    };
    if negative {
        text.insert(0, '-');
    }
    text
}

fn shortest_decimal_components(number: f64) -> (String, i32) {
    let raw = number.to_string();
    let (mantissa, explicit_exponent) = raw.split_once('e').or_else(|| raw.split_once('E')).map_or(
        (raw.as_str(), 0),
        |(mantissa, exponent)| {
            (
                mantissa,
                exponent
                    .parse::<i32>()
                    .expect("f64 exponent emitted by Display is valid"),
            )
        },
    );
    let decimal_position = mantissa.find('.').unwrap_or(mantissa.len());
    let raw_digits: String = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(char::from)
        .collect();
    let first = raw_digits
        .bytes()
        .position(|digit| digit != b'0')
        .expect("non-zero f64 display contains a non-zero digit");
    let end = raw_digits
        .bytes()
        .rposition(|digit| digit != b'0')
        .expect("non-zero f64 display contains a non-zero digit")
        + 1;
    let exponent = explicit_exponent + decimal_position as i32 - first as i32 - 1;
    (raw_digits[first..end].to_owned(), exponent)
}

fn format_fixed(number: f64, digits: usize) -> String {
    let negative = number < 0.0;
    let (numerator, shift) = exact_binary_rational(number.abs());
    let rounded = round_ratio(
        numerator * decimal_power(digits as u32),
        BigUint::one() << shift as usize,
    );
    let mut text = rounded.to_string();
    if digits != 0 {
        if text.len() <= digits {
            text.insert_str(0, &"0".repeat(digits + 1 - text.len()));
        }
        text.insert(text.len() - digits, '.');
    }
    if negative {
        text.insert(0, '-');
    }
    text
}

fn format_exponential(number: f64, fraction_digits: Option<usize>) -> String {
    if number == 0.0 {
        let digits = fraction_digits.unwrap_or(0);
        let coefficient = if digits == 0 {
            "0".into()
        } else {
            format!("0.{}", "0".repeat(digits))
        };
        return format!("{coefficient}e+0");
    }
    let negative = number < 0.0;
    let (digits, exponent) = if let Some(fraction_digits) = fraction_digits {
        rounded_significant_digits(number.abs(), fraction_digits + 1)
    } else {
        shortest_decimal_components(number.abs())
    };
    let mut text = scientific_notation(&digits, exponent);
    if negative {
        text.insert(0, '-');
    }
    text
}

fn format_precision(number: f64, precision: usize) -> String {
    if number == 0.0 {
        return if precision == 1 {
            "0".into()
        } else {
            format!("0.{}", "0".repeat(precision - 1))
        };
    }
    let negative = number < 0.0;
    let (digits, exponent) = rounded_significant_digits(number.abs(), precision);
    let mut text = if exponent < -6 || exponent >= precision as i32 {
        scientific_notation(&digits, exponent)
    } else if exponent < 0 {
        format!("0.{}{}", "0".repeat((-exponent - 1) as usize), digits)
    } else {
        let position = exponent as usize + 1;
        if position == digits.len() {
            digits
        } else {
            let mut text = digits;
            text.insert(position, '.');
            text
        }
    };
    if negative {
        text.insert(0, '-');
    }
    text
}

fn rounded_significant_digits(number: f64, precision: usize) -> (String, i32) {
    let (numerator, shift) = exact_binary_rational(number);
    let mut exponent = decimal_exponent(&numerator, shift, number);
    let scale = precision as i32 - 1 - exponent;
    let denominator = BigUint::one() << shift as usize;
    let mut rounded = if scale >= 0 {
        round_ratio(numerator * decimal_power(scale as u32), denominator)
    } else {
        round_ratio(numerator, denominator * decimal_power((-scale) as u32))
    };
    let limit = decimal_power(precision as u32);
    if rounded >= limit {
        rounded /= 10_u8;
        exponent += 1;
    }
    let mut digits = rounded.to_string();
    if digits.len() < precision {
        digits.insert_str(0, &"0".repeat(precision - digits.len()));
    }
    (digits, exponent)
}

fn decimal_exponent(numerator: &BigUint, shift: u32, number: f64) -> i32 {
    let mut exponent = number.log10().floor() as i32;
    while compare_to_decimal_power(numerator, shift, exponent) == Ordering::Less {
        exponent -= 1;
    }
    while compare_to_decimal_power(numerator, shift, exponent + 1) != Ordering::Less {
        exponent += 1;
    }
    exponent
}

fn compare_to_decimal_power(numerator: &BigUint, shift: u32, exponent: i32) -> Ordering {
    if exponent >= 0 {
        numerator.cmp(&(decimal_power(exponent as u32) << shift as usize))
    } else {
        (numerator * decimal_power((-exponent) as u32)).cmp(&(BigUint::one() << shift as usize))
    }
}

fn round_ratio(numerator: BigUint, denominator: BigUint) -> BigUint {
    let quotient = &numerator / &denominator;
    let remainder = numerator % &denominator;
    if (&remainder << 1_usize) >= denominator {
        quotient + BigUint::one()
    } else {
        quotient
    }
}

fn decimal_power(exponent: u32) -> BigUint {
    BigUint::from(10_u8).pow(exponent)
}

fn scientific_notation(digits: &str, exponent: i32) -> String {
    let mut coefficient = digits[..1].to_owned();
    if digits.len() > 1 {
        coefficient.push('.');
        coefficient.push_str(&digits[1..]);
    }
    let sign = if exponent >= 0 { '+' } else { '-' };
    format!("{coefficient}e{sign}{}", exponent.unsigned_abs())
}

fn parse_int(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let input = match to_string_coerced(ctx, state, input) {
        Ok(input) => input,
        Err(exception) => return exception,
    };
    let mut input = input
        .trim_start_matches(|character: char| character.is_whitespace() || character == '\u{FEFF}');
    let sign = if let Some(rest) = input.strip_prefix('-') {
        input = rest;
        -1.0
    } else {
        if let Some(rest) = input.strip_prefix('+') {
            input = rest;
        }
        1.0
    };
    let mut radix = args
        .get(1)
        .and_then(|radix| to_number(state, *radix))
        .map(to_int32)
        .unwrap_or(0);
    let strip_prefix = radix == 0 || radix == 16;
    if radix == 0 {
        radix = 10;
    }
    if strip_prefix
        && let Some(rest) = input
            .strip_prefix("0x")
            .or_else(|| input.strip_prefix("0X"))
    {
        input = rest;
        radix = 16;
    }
    if !(2..=36).contains(&radix) {
        return value::encode_f64(f64::NAN);
    }
    let mut parsed = 0.0;
    let mut consumed = false;
    for character in input.chars() {
        let Some(digit) = character.to_digit(radix as u32) else {
            break;
        };
        consumed = true;
        parsed = parsed * f64::from(radix) + f64::from(digit);
    }
    value::encode_f64(if consumed { sign * parsed } else { f64::NAN })
}

fn parse_float(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let input = match to_string_coerced(ctx, state, input) {
        Ok(input) => input,
        Err(exception) => return exception,
    };
    let mut prefix = input
        .trim_start_matches(|character: char| character.is_whitespace() || character == '\u{FEFF}');
    while !prefix.is_empty() {
        if let Ok(number) = prefix.parse::<f64>() {
            return value::encode_f64(number);
        }
        let Some((index, _)) = prefix.char_indices().next_back() else {
            break;
        };
        prefix = &prefix[..index];
    }
    value::encode_f64(f64::NAN)
}

fn format_radix(number: f64, radix: u32) -> String {
    if number == 0.0 {
        return "0".into();
    }
    let negative = number.is_sign_negative();
    let number = number.abs();
    let (numerator, denominator_shift) = exact_binary_rational(number);
    let previous = f64::from_bits(number.to_bits() - 1);
    let lower = midpoint(
        exact_binary_rational(previous),
        (numerator.clone(), denominator_shift),
    );
    let next = f64::from_bits(number.to_bits() + 1);
    let upper = if next.is_finite() {
        midpoint(
            (numerator.clone(), denominator_shift),
            exact_binary_rational(next),
        )
    } else {
        upper_from_previous(
            (numerator.clone(), denominator_shift),
            exact_binary_rational(previous),
        )
    };
    let radix_value = BigUint::from(radix);
    let mut scale = BigUint::one();
    let inclusive = number.to_bits() & 1 == 0;
    let mut fractional_digits = 0_usize;
    loop {
        let minimum = lower_integer_bound(&lower, &scale, inclusive);
        let maximum = upper_integer_bound(&upper, &scale, inclusive);
        if minimum <= maximum {
            let rounded = rounded_scaled(&numerator, denominator_shift, &scale);
            let digits = rounded.max(minimum).min(maximum);
            let mut text = format_biguint_radix(digits, radix);
            if fractional_digits != 0 {
                if text.len() <= fractional_digits {
                    text.insert_str(0, &"0".repeat(fractional_digits + 1 - text.len()));
                }
                text.insert(text.len() - fractional_digits, '.');
            }
            if negative {
                text.insert(0, '-');
            }
            return text;
        }
        scale *= &radix_value;
        fractional_digits += 1;
    }
}

fn exact_binary_rational(number: f64) -> (BigUint, u32) {
    let bits = number.to_bits();
    let exponent_bits = ((bits >> 52) & 0x7FF) as i32;
    let fraction = bits & ((1_u64 << 52) - 1);
    let (significand, exponent) = if exponent_bits == 0 {
        (fraction, -1074)
    } else {
        ((1_u64 << 52) | fraction, exponent_bits - 1023 - 52)
    };
    let mut numerator = BigUint::from(significand);
    if exponent >= 0 {
        numerator <<= exponent as usize;
        (numerator, 0)
    } else {
        (numerator, (-exponent) as u32)
    }
}

fn midpoint(left: (BigUint, u32), right: (BigUint, u32)) -> (BigUint, u32) {
    let shift = left.1.max(right.1);
    let numerator = (left.0 << (shift - left.1) as usize) + (right.0 << (shift - right.1) as usize);
    (numerator, shift + 1)
}

fn upper_from_previous(value: (BigUint, u32), previous: (BigUint, u32)) -> (BigUint, u32) {
    let shift = value.1.max(previous.1);
    let value = value.0 << (shift - value.1) as usize;
    let previous = previous.0 << (shift - previous.1) as usize;
    ((&value << 1_usize) + value - previous, shift + 1)
}

fn lower_integer_bound(bound: &(BigUint, u32), scale: &BigUint, inclusive: bool) -> BigUint {
    let scaled = &bound.0 * scale;
    let denominator = BigUint::one() << bound.1 as usize;
    let quotient = &scaled / &denominator;
    let remainder = scaled % denominator;
    if remainder.is_zero() && inclusive {
        quotient
    } else {
        quotient + BigUint::one()
    }
}

fn upper_integer_bound(bound: &(BigUint, u32), scale: &BigUint, inclusive: bool) -> BigUint {
    let scaled = &bound.0 * scale;
    let denominator = BigUint::one() << bound.1 as usize;
    let quotient = &scaled / &denominator;
    let remainder = scaled % denominator;
    if remainder.is_zero() && !inclusive {
        quotient - BigUint::one()
    } else {
        quotient
    }
}

fn rounded_scaled(numerator: &BigUint, shift: u32, scale: &BigUint) -> BigUint {
    if shift == 0 {
        return numerator * scale;
    }
    let scaled = numerator * scale;
    let denominator = BigUint::one() << shift as usize;
    let quotient = &scaled / &denominator;
    let remainder = scaled % &denominator;
    let doubled = &remainder << 1_usize;
    if doubled > denominator || doubled == denominator && quotient.bit(0) {
        quotient + BigUint::one()
    } else {
        quotient
    }
}

fn format_biguint_radix(mut value: BigUint, radix: u32) -> String {
    if value.is_zero() {
        return "0".into();
    }
    let radix_value = BigUint::from(radix);
    let mut digits = Vec::new();
    while !value.is_zero() {
        let digit = (&value % &radix_value)
            .to_u8()
            .expect("radix remainder fits in u8");
        digits.push(if digit < 10 {
            b'0' + digit
        } else {
            b'a' + digit - 10
        });
        value /= &radix_value;
    }
    digits.reverse();
    String::from_utf8(digits).expect("radix digits are ASCII")
}
