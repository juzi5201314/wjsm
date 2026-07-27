//! 相等与关系比较算法。

use num_bigint::{BigInt, Sign};
use num_traits::{FromPrimitive, ToPrimitive};
use wjsm_host::{ExecContext, ToPrimitiveHintKind, Value};
use wjsm_ir::value;

const TYPE_NUMBER: u8 = 0;
const TYPE_STRING: u8 = 1;
const TYPE_UNDEFINED: u8 = 2;
const TYPE_NULL: u8 = 3;
const TYPE_BOOLEAN: u8 = 4;
const TYPE_OBJECT: u8 = 5;
const TYPE_BIGINT: u8 = 6;
const TYPE_SYMBOL: u8 = 7;

#[inline]
fn type_tag(val: Value) -> u8 {
    if value::is_f64(val) {
        TYPE_NUMBER
    } else if value::is_string(val) {
        TYPE_STRING
    } else if value::is_undefined(val) {
        TYPE_UNDEFINED
    } else if value::is_null(val) {
        TYPE_NULL
    } else if value::is_bool(val) {
        TYPE_BOOLEAN
    } else if value::is_bigint(val) {
        TYPE_BIGINT
    } else if value::is_symbol(val) {
        TYPE_SYMBOL
    } else {
        TYPE_OBJECT
    }
}
#[inline]
pub fn strict_eq_impl<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    let kind = type_tag(a);
    if kind != type_tag(b) {
        return value::encode_bool(false);
    }
    let equal = match kind {
        TYPE_NUMBER => {
            let a = value::decode_f64(a);
            let b = value::decode_f64(b);
            !a.is_nan() && !b.is_nan() && a == b
        }
        TYPE_STRING => ctx.string_values_equal(a, b),
        TYPE_UNDEFINED | TYPE_NULL => true,
        TYPE_BOOLEAN => value::decode_bool(a) == value::decode_bool(b),
        TYPE_BIGINT => ctx
            .read_bigint(a)
            .zip(ctx.read_bigint(b))
            .is_some_and(|(a, b)| a == b),
        TYPE_SYMBOL => a == b,
        _ if a == b => true,
        _ if value::is_function(a) && value::is_closure(b) => {
            ctx.function_closure_identity_eq(a, b)
        }
        _ if value::is_closure(a) && value::is_function(b) => {
            ctx.function_closure_identity_eq(b, a)
        }
        _ => false,
    };
    value::encode_bool(equal)
}

fn bigint_number_equal<E: ExecContext>(ctx: &mut E, bigint: Value, number: Value) -> bool {
    let number = value::decode_f64(number);
    if !number.is_finite() || number.fract() != 0.0 {
        return false;
    }
    BigInt::from_f64(number)
        .zip(ctx.read_bigint(bigint))
        .is_some_and(|(number, bigint)| number == bigint)
}

fn bigint_string_equal<E: ExecContext>(ctx: &mut E, bigint: Value, string: Value) -> bool {
    let string = ctx.get_runtime_string(string).to_utf8_lossy();
    string
        .trim_end_matches('\0')
        .parse::<BigInt>()
        .ok()
        .zip(ctx.read_bigint(bigint))
        .is_some_and(|(string, bigint)| string == bigint)
}

#[inline]
fn is_object_coercible_primitive(val: Value) -> bool {
    value::is_string(val) || value::is_f64(val) || value::is_bigint(val) || value::is_symbol(val)
}
#[inline]
pub fn abstract_eq_impl<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    let mut x = a;
    let mut y = b;
    loop {
        if type_tag(x) == type_tag(y) {
            return strict_eq_impl(ctx, x, y);
        }
        if (value::is_null(x) && value::is_undefined(y))
            || (value::is_undefined(x) && value::is_null(y))
        {
            return value::encode_bool(true);
        }
        if value::is_f64(x) && value::is_string(y) {
            y = ctx.to_number(y);
            continue;
        }
        if value::is_string(x) && value::is_f64(y) {
            x = ctx.to_number(x);
            continue;
        }
        if value::is_bool(x) {
            x = ctx.to_number(x);
            continue;
        }
        if value::is_bool(y) {
            y = ctx.to_number(y);
            continue;
        }
        if type_tag(x) == TYPE_OBJECT && is_object_coercible_primitive(y) {
            x = ctx.to_primitive_hinted(x, ToPrimitiveHintKind::Default);
            if value::is_exception(x) {
                return x;
            }
            continue;
        }
        if is_object_coercible_primitive(x) && type_tag(y) == TYPE_OBJECT {
            y = ctx.to_primitive_hinted(y, ToPrimitiveHintKind::Default);
            if value::is_exception(y) {
                return y;
            }
            continue;
        }
        if value::is_bigint(x) && value::is_f64(y) {
            return value::encode_bool(bigint_number_equal(ctx, x, y));
        }
        if value::is_f64(x) && value::is_bigint(y) {
            return value::encode_bool(bigint_number_equal(ctx, y, x));
        }
        if value::is_bigint(x) && value::is_string(y) {
            return value::encode_bool(bigint_string_equal(ctx, x, y));
        }
        if value::is_string(x) && value::is_bigint(y) {
            return value::encode_bool(bigint_string_equal(ctx, y, x));
        }
        return value::encode_bool(false);
    }
}

fn number_bigint_less_than(number: f64, bigint: &BigInt, bigint_is_left: bool) -> bool {
    let truncated = number.trunc();
    if number.is_finite() && number.abs() <= (1_i64 << 53) as f64 {
        let integer = number as i64;
        if (number - integer as f64).abs() < 1.0 {
            let integer = BigInt::from(integer);
            return if number == truncated {
                if bigint_is_left {
                    bigint < &integer
                } else {
                    integer < *bigint
                }
            } else if bigint_is_left {
                bigint <= &integer
            } else {
                integer <= *bigint
            };
        }
    }
    match bigint.to_f64() {
        Some(bigint) if bigint_is_left => bigint < number,
        Some(bigint) => number < bigint,
        None if bigint.sign() == Sign::Minus => bigint_is_left,
        None => !bigint_is_left,
    }
}
#[inline]
pub fn abstract_compare_impl<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    let a = ctx.to_primitive_hinted(a, ToPrimitiveHintKind::Number);
    if value::is_exception(a) {
        return a;
    }
    let b = ctx.to_primitive_hinted(b, ToPrimitiveHintKind::Number);
    if value::is_exception(b) {
        return b;
    }
    if value::is_string(a) && value::is_string(b) {
        return value::encode_bool(ctx.string_lt(a, b));
    }

    let a = if value::is_bigint(a) {
        a
    } else {
        ctx.to_number(a)
    };
    let b = if value::is_bigint(b) {
        b
    } else {
        ctx.to_number(b)
    };
    if value::is_exception(a) {
        return a;
    }
    if value::is_exception(b) {
        return b;
    }
    if value::is_bigint(a) && value::is_bigint(b) {
        return value::encode_bool(
            ctx.read_bigint(a)
                .zip(ctx.read_bigint(b))
                .is_some_and(|(a, b)| a < b),
        );
    }
    if value::is_bigint(a) || value::is_bigint(b) {
        let (bigint, number) = if value::is_bigint(a) { (a, b) } else { (b, a) };
        let number = value::decode_f64(number);
        if number.is_nan() {
            return value::encode_bool(false);
        }
        if number.is_infinite() {
            let bigint_is_left = value::is_bigint(a);
            return value::encode_bool(if number.is_sign_positive() {
                bigint_is_left
            } else {
                !bigint_is_left
            });
        }
        let Some(bigint) = ctx.read_bigint(bigint) else {
            return value::encode_bool(false);
        };
        return value::encode_bool(number_bigint_less_than(
            number,
            &bigint,
            value::is_bigint(a),
        ));
    }

    let a = value::decode_f64(a);
    let b = value::decode_f64(b);
    value::encode_bool(!a.is_nan() && !b.is_nan() && a < b)
}
