//! BigInt / Symbol / RegExp 基础宿主 builtin。

use num_bigint::{BigInt, Sign};
use num_traits::{ToPrimitive, Zero};
use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

fn bigint_binary<E: ExecContext>(
    ctx: &mut E,
    a: Value,
    b: Value,
    op: impl Fn(&BigInt, &BigInt) -> BigInt,
) -> Value {
    match (ctx.read_bigint(a), ctx.read_bigint(b)) {
        (Some(av), Some(bv)) => ctx.store_bigint(op(&av, &bv)),
        _ => value::encode_undefined(),
    }
}

fn bigint_shift_amount(y: &BigInt) -> Result<u64, &'static str> {
    let modulus = BigInt::from(1u64) << 64;
    let reduced: BigInt = y % &modulus;
    if reduced.sign() == Sign::Minus {
        return Err("RangeError: BigInt shift amount must be non-negative");
    }
    reduced
        .to_u64()
        .ok_or("RangeError: BigInt shift amount too large")
}

pub fn bigint_from_literal<E: ExecContext>(ctx: &mut E, ptr: i32, _len: i32) -> Value {
    let s = ctx.read_memory_string(ptr as u32, None);
    let trimmed = s.trim_end_matches('\0');
    match trimmed.parse::<BigInt>() {
        Ok(bigint) => ctx.store_bigint(bigint),
        Err(_) => value::encode_undefined(),
    }
}

pub fn bigint_add<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    bigint_binary(ctx, a, b, |x, y| x + y)
}
pub fn bigint_sub<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    bigint_binary(ctx, a, b, |x, y| x - y)
}
pub fn bigint_mul<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    bigint_binary(ctx, a, b, |x, y| x * y)
}

pub fn bigint_div<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    match (ctx.read_bigint(a), ctx.read_bigint(b)) {
        (Some(x), Some(y)) => {
            if y.is_zero() {
                ctx.set_last_error("RangeError: BigInt division by zero".to_string());
                return value::encode_undefined();
            }
            ctx.store_bigint(x / y)
        }
        _ => value::encode_undefined(),
    }
}

pub fn bigint_mod<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    match (ctx.read_bigint(a), ctx.read_bigint(b)) {
        (Some(x), Some(y)) => {
            if y.is_zero() {
                ctx.set_last_error("RangeError: BigInt division by zero".to_string());
                return value::encode_undefined();
            }
            ctx.store_bigint(x % y)
        }
        _ => value::encode_undefined(),
    }
}

pub fn bigint_pow<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    match (ctx.read_bigint(a), ctx.read_bigint(b)) {
        (Some(x), Some(y)) => {
            if y.sign() == Sign::Minus {
                ctx.set_last_error("RangeError: BigInt exponent must be non-negative".to_string());
                return value::encode_undefined();
            }
            let exp = match y.to_u32() {
                Some(e) => e,
                None => {
                    ctx.set_last_error("RangeError: BigInt exponent too large".to_string());
                    return value::encode_undefined();
                }
            };
            ctx.store_bigint(x.pow(exp))
        }
        _ => value::encode_undefined(),
    }
}

pub fn bigint_neg<E: ExecContext>(ctx: &mut E, a: Value) -> Value {
    match ctx.read_bigint(a) {
        Some(av) => ctx.store_bigint(-av),
        None => value::encode_undefined(),
    }
}

pub fn bigint_bit_and<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    bigint_binary(ctx, a, b, |x, y| x & y)
}
pub fn bigint_bit_or<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    bigint_binary(ctx, a, b, |x, y| x | y)
}
pub fn bigint_bit_xor<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    bigint_binary(ctx, a, b, |x, y| x ^ y)
}

pub fn bigint_shl<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    match (ctx.read_bigint(a), ctx.read_bigint(b)) {
        (Some(x), Some(y)) => match bigint_shift_amount(&y) {
            Ok(shift) => ctx.store_bigint(x << shift),
            Err(msg) => {
                ctx.set_last_error(msg.to_string());
                value::encode_undefined()
            }
        },
        _ => value::encode_undefined(),
    }
}

pub fn bigint_shr<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    match (ctx.read_bigint(a), ctx.read_bigint(b)) {
        (Some(x), Some(y)) => match bigint_shift_amount(&y) {
            Ok(shift) => ctx.store_bigint(x >> shift),
            Err(msg) => {
                ctx.set_last_error(msg.to_string());
                value::encode_undefined()
            }
        },
        _ => value::encode_undefined(),
    }
}

pub fn bigint_bit_not<E: ExecContext>(ctx: &mut E, a: Value) -> Value {
    match ctx.read_bigint(a) {
        Some(av) => ctx.store_bigint(!av),
        None => value::encode_undefined(),
    }
}

pub fn bigint_eq<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    let eq = match (ctx.read_bigint(a), ctx.read_bigint(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    };
    value::encode_bool(eq)
}

pub fn bigint_cmp<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    use std::cmp::Ordering;
    let cmp = match (ctx.read_bigint(a), ctx.read_bigint(b)) {
        (Some(x), Some(y)) => match x.cmp(&y) {
            Ordering::Less => -1.0f64,
            Ordering::Equal => 0.0f64,
            Ordering::Greater => 1.0f64,
        },
        _ => f64::NAN,
    };
    cmp.to_bits() as i64
}

pub fn symbol_create<E: ExecContext>(ctx: &mut E, desc: Value) -> Value {
    let description = if value::is_undefined(desc) {
        None
    } else if value::is_string(desc) {
        Some(ctx.read_string_utf8_lossy(desc))
    } else {
        Some(ctx.render_value(desc))
    };
    ctx.create_symbol(description, None)
}

pub fn symbol_for<E: ExecContext>(ctx: &mut E, key: Value) -> Value {
    let key_str = match ctx.value_to_key_string(key) {
        Ok(s) => s,
        Err(exception) => return exception,
    };
    if let Some(existing) = ctx.find_global_symbol(&key_str) {
        return existing;
    }
    ctx.create_symbol(Some(key_str.clone()), Some(key_str))
}

pub fn symbol_key_for<E: ExecContext>(ctx: &mut E, sym: Value) -> Value {
    if !value::is_symbol(sym) {
        return ctx.make_type_error("TypeError: sym is not a Symbol");
    }
    match ctx.symbol_entry(sym) {
        Some((_, Some(key))) => ctx.store_string_owned(key),
        _ => value::encode_undefined(),
    }
}

pub fn symbol_well_known<E: ExecContext>(ctx: &mut E, id: i32) -> Value {
    ctx.symbol_well_known(id)
}

pub fn regex_create<E: ExecContext>(
    ctx: &mut E,
    pat_ptr: i32,
    pat_len: i32,
    flags_ptr: i32,
    flags_len: i32,
) -> Value {
    let pattern = ctx.read_memory_string(pat_ptr as u32, Some(pat_len as u32));
    let flags = ctx.read_memory_string(flags_ptr as u32, Some(flags_len as u32));
    ctx.regexp_create(pattern, flags)
}

pub fn regex_test<E: ExecContext>(ctx: &mut E, regex_val: Value, str_val: Value) -> Value {
    ctx.regexp_test(regex_val, str_val)
}

pub fn regex_exec<E: ExecContext>(ctx: &mut E, regex_val: Value, str_val: Value) -> Value {
    ctx.regexp_exec(regex_val, str_val)
}
