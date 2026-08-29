use num_bigint::{BigInt, Sign};
use num_traits::{ToPrimitive, Zero};
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::{
    modules,
    runtime::{fail_dispatch, range_error, to_number},
};
use crate::NativeAgentState;

pub(super) fn dispatch_bigint(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::BigIntFromLiteral => from_value(ctx, state, args),
        Builtin::BigIntAdd => binary(ctx, state, args, |left, right| Some(left + right)),
        Builtin::BigIntSub => binary(ctx, state, args, |left, right| Some(left - right)),
        Builtin::BigIntMul => binary(ctx, state, args, |left, right| Some(left * right)),
        Builtin::BigIntDiv => divide(ctx, state, args, false),
        Builtin::BigIntMod => divide(ctx, state, args, true),
        Builtin::BigIntPow => pow(ctx, state, args),
        Builtin::BigIntNeg => unary(ctx, state, args, |input| -input),
        Builtin::BigIntBitAnd => binary(ctx, state, args, |left, right| Some(left & right)),
        Builtin::BigIntBitOr => binary(ctx, state, args, |left, right| Some(left | right)),
        Builtin::BigIntBitXor => binary(ctx, state, args, |left, right| Some(left ^ right)),
        Builtin::BigIntShl => binary(ctx, state, args, |left, right| {
            shift_amount(&right).map(|shift| left << shift)
        }),
        Builtin::BigIntShr => binary(ctx, state, args, |left, right| {
            shift_amount(&right).map(|shift| left >> shift)
        }),
        Builtin::BigIntBitNot => unary(ctx, state, args, |input| !input),
        Builtin::BigIntEq => compare(ctx, state, args, true),
        Builtin::BigIntCmp => compare(ctx, state, args, false),
        Builtin::BigIntProtoToString => proto_to_string(ctx, state, args),
        Builtin::BigIntProtoValueOf => proto_value_of(ctx, state, args),
        _ => return None,
    })
}

pub(super) fn operands(state: &NativeAgentState, args: &[i64]) -> Option<(BigInt, BigInt)> {
    let [left, right] = args else { return None };
    Some((read(state, *left)?, read(state, *right)?))
}

pub(super) fn store(state: &mut NativeAgentState, input: BigInt) -> Option<i64> {
    state.intern_text(input.to_string(), value::TAG_BIGINT)
}

pub(super) fn read(state: &NativeAgentState, encoded: i64) -> Option<BigInt> {
    if !value::is_bigint(encoded) {
        return None;
    }
    state.string_owned(encoded)?.to_utf8()?.parse().ok()
}
fn proto_to_string(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(integer) = read(state, receiver) else {
        return type_error(
            ctx,
            state,
            "BigInt.prototype.toString called on incompatible receiver",
        );
    };
    let radix = match args.get(1).copied() {
        None => 10,
        Some(encoded) if value::is_undefined(encoded) => 10,
        Some(encoded) => {
            let Some(number) = to_number(state, encoded) else {
                return type_error(ctx, state, "BigInt radix cannot be converted to a number");
            };
            number.trunc().to_u32().unwrap_or(0)
        }
    };
    if !(2..=36).contains(&radix) {
        return range_error(ctx, state, "BigInt radix must be between 2 and 36");
    }
    state
        .intern_text(integer.to_str_radix(radix), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn proto_value_of(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args
        .first()
        .copied()
        .filter(|receiver| value::is_bigint(*receiver))
    else {
        return type_error(
            ctx,
            state,
            "BigInt.prototype.valueOf called on incompatible receiver",
        );
    };
    receiver
}

fn from_value(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    // §21.2.1.1 步骤 1：NewTarget 已定义时抛 TypeError（`new BigInt()`），
    // 文案对齐 V8/Node；BigInt 字面量的直连站点复用外层激活，不受影响。
    if super::runtime::is_builtin_construct_call(state, Builtin::BigIntFromLiteral) {
        return type_error(ctx, state, "BigInt is not a constructor");
    }
    let Some(input) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let parsed = if value::is_bigint(input) {
        read(state, input)
    } else if value::is_f64(input) {
        let number = value::decode_f64(input);
        (number.is_finite() && number.fract() == 0.0)
            .then(|| number.to_i128())
            .flatten()
            .map(BigInt::from)
    } else if value::is_bool(input) {
        Some(BigInt::from(u8::from(value::decode_bool(input))))
    } else if value::is_string(input) {
        state
            .string_owned(input)
            .and_then(|text| text.to_utf8())
            .and_then(|text| text.trim().parse().ok())
    } else {
        None
    };
    parsed
        .and_then(|value| store(state, value))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn binary(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    operation: impl FnOnce(BigInt, BigInt) -> Option<BigInt>,
) -> i64 {
    let [left, right] = args else {
        return fail_dispatch(ctx);
    };
    let (Some(left), Some(right)) = (read(state, *left), read(state, *right)) else {
        return type_error(
            ctx,
            state,
            "Cannot mix BigInt and other types, use explicit conversions",
        );
    };
    operation(left, right)
        .and_then(|result| store(state, result))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn divide(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    remainder: bool,
) -> i64 {
    let (left, right) = match typed_operands(ctx, state, args) {
        Ok(operands) => operands,
        Err(exception) => return exception,
    };
    if right.is_zero() {
        return range_error(ctx, state, "Division by zero");
    }
    store(
        state,
        if remainder {
            left % right
        } else {
            left / right
        },
    )
    .unwrap_or_else(|| fail_dispatch(ctx))
}

fn pow(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let (left, right) = match typed_operands(ctx, state, args) {
        Ok(operands) => operands,
        Err(exception) => return exception,
    };
    if right.sign() == Sign::Minus {
        return range_error(ctx, state, "BigInt exponent must be non-negative");
    }
    let Some(exponent) = right.to_u32() else {
        return range_error(ctx, state, "BigInt exponent is too large");
    };
    store(state, left.pow(exponent)).unwrap_or_else(|| fail_dispatch(ctx))
}

fn typed_operands(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> Result<(BigInt, BigInt), i64> {
    let [left, right] = args else {
        return Err(fail_dispatch(ctx));
    };
    let (Some(left), Some(right)) = (read(state, *left), read(state, *right)) else {
        return Err(type_error(
            ctx,
            state,
            "Cannot mix BigInt and other types, use explicit conversions",
        ));
    };
    Ok((left, right))
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    modules::named_error_object(state, "TypeError", message.to_owned())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn unary(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    operation: impl FnOnce(BigInt) -> BigInt,
) -> i64 {
    args.first()
        .and_then(|input| read(state, *input))
        .map(operation)
        .and_then(|result| store(state, result))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn compare(
    ctx: &mut NativeVmContext,
    state: &NativeAgentState,
    args: &[i64],
    equality: bool,
) -> i64 {
    let Some((left, right)) = operands(state, args) else {
        return fail_dispatch(ctx);
    };
    if equality {
        value::encode_bool(left == right)
    } else {
        value::encode_f64(match left.cmp(&right) {
            std::cmp::Ordering::Less => -1.0,
            std::cmp::Ordering::Equal => 0.0,
            std::cmp::Ordering::Greater => 1.0,
        })
    }
}

fn shift_amount(input: &BigInt) -> Option<u64> {
    let modulus = BigInt::from(1_u128 << 64);
    let reduced = input % &modulus;
    let normalized = if reduced.sign() == Sign::Minus {
        reduced + modulus
    } else {
        reduced
    };
    normalized.to_u64()
}
