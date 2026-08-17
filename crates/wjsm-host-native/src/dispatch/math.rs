use rand::Rng;

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, object_handle, to_int32, to_number_coerced, to_uint32};
use crate::NativeAgentState;

// ── typed f64 math thunk ────────────────────────────────────────────────
//
// generated code 在 `infer_f64_values` 证明实参为 f64 时直接以 `(F64)->F64` /
// `(F64,F64)->F64` 调用这些 thunk，跳过 `CallBuiltin` → `call_dispatcher` 的
// arena 打包 + builtin 跳表 + `to_number_coerced` 往返。thunk 必须与 dispatcher
// 路径使用同一 Rust libm 实现，保证同一输入在 typed / untyped 路径下位级一致
// （NaN 统一由 `box_f64_result` 规范化）。不依赖平台 libc 符号命名差异。

pub unsafe extern "C" fn native_math_acos(value: f64) -> f64 {
    f64::acos(value)
}
pub unsafe extern "C" fn native_math_acosh(value: f64) -> f64 {
    f64::acosh(value)
}
pub unsafe extern "C" fn native_math_asin(value: f64) -> f64 {
    f64::asin(value)
}
pub unsafe extern "C" fn native_math_asinh(value: f64) -> f64 {
    f64::asinh(value)
}
pub unsafe extern "C" fn native_math_atan(value: f64) -> f64 {
    f64::atan(value)
}
pub unsafe extern "C" fn native_math_atanh(value: f64) -> f64 {
    f64::atanh(value)
}
pub unsafe extern "C" fn native_math_atan2(y: f64, x: f64) -> f64 {
    f64::atan2(y, x)
}
pub unsafe extern "C" fn native_math_cbrt(value: f64) -> f64 {
    f64::cbrt(value)
}
pub unsafe extern "C" fn native_math_cos(value: f64) -> f64 {
    f64::cos(value)
}
pub unsafe extern "C" fn native_math_cosh(value: f64) -> f64 {
    f64::cosh(value)
}
pub unsafe extern "C" fn native_math_exp(value: f64) -> f64 {
    f64::exp(value)
}
pub unsafe extern "C" fn native_math_expm1(value: f64) -> f64 {
    f64::exp_m1(value)
}
pub unsafe extern "C" fn native_math_log(value: f64) -> f64 {
    f64::ln(value)
}
pub unsafe extern "C" fn native_math_log1p(value: f64) -> f64 {
    f64::ln_1p(value)
}
pub unsafe extern "C" fn native_math_log10(value: f64) -> f64 {
    f64::log10(value)
}
pub unsafe extern "C" fn native_math_log2(value: f64) -> f64 {
    f64::log2(value)
}
pub unsafe extern "C" fn native_math_sin(value: f64) -> f64 {
    f64::sin(value)
}
pub unsafe extern "C" fn native_math_sinh(value: f64) -> f64 {
    f64::sinh(value)
}
pub unsafe extern "C" fn native_math_tan(value: f64) -> f64 {
    f64::tan(value)
}
pub unsafe extern "C" fn native_math_tanh(value: f64) -> f64 {
    f64::tanh(value)
}
pub unsafe extern "C" fn native_math_pow(base: f64, exponent: f64) -> f64 {
    f64::powf(base, exponent)
}

pub(super) fn dispatch_math(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    let unary: Option<fn(f64) -> f64> = match builtin {
        Builtin::MathAbs => Some(f64::abs as fn(f64) -> f64),
        Builtin::MathAcos => Some(f64::acos),
        Builtin::MathAcosh => Some(f64::acosh),
        Builtin::MathAsin => Some(f64::asin),
        Builtin::MathAsinh => Some(f64::asinh),
        Builtin::MathAtan => Some(f64::atan),
        Builtin::MathAtanh => Some(f64::atanh),
        Builtin::MathCbrt => Some(f64::cbrt),
        Builtin::MathCeil => Some(f64::ceil),
        Builtin::MathCos => Some(f64::cos),
        Builtin::MathCosh => Some(f64::cosh),
        Builtin::MathExp => Some(f64::exp),
        Builtin::MathExpm1 => Some(f64::exp_m1),
        Builtin::MathFloor => Some(f64::floor),
        Builtin::MathLog => Some(f64::ln),
        Builtin::MathLog1p => Some(f64::ln_1p),
        Builtin::MathLog10 => Some(f64::log10),
        Builtin::MathLog2 => Some(f64::log2),
        Builtin::MathSin => Some(f64::sin),
        Builtin::MathSinh => Some(f64::sinh),
        Builtin::MathSqrt => Some(f64::sqrt),
        Builtin::MathTan => Some(f64::tan),
        Builtin::MathTanh => Some(f64::tanh),
        Builtin::MathTrunc => Some(f64::trunc),
        _ => None,
    };
    if let Some(operation) = unary {
        return Some(unary_number(ctx, state, args, operation));
    }

    Some(match builtin {
        Builtin::MathAtan2 => binary_number(ctx, state, args, f64::atan2),
        Builtin::MathPow => binary_number(ctx, state, args, f64::powf),
        Builtin::MathRandom => value::encode_f64(rand::thread_rng().gen_range(0.0..1.0)),
        Builtin::MathClz32 => match first_number(ctx, state, args) {
            Ok(number) => value::encode_f64(f64::from(to_uint32(number).leading_zeros())),
            Err(exception) => exception,
        },
        Builtin::MathFround => match first_number(ctx, state, args) {
            Ok(number) => value::encode_f64(f64::from(number as f32)),
            Err(exception) => exception,
        },
        Builtin::MathImul => {
            let [left, right] = args else {
                return Some(fail_dispatch(ctx));
            };
            let left = match to_number_coerced(ctx, state, *left) {
                Ok(number) => number,
                Err(exception) => return Some(exception),
            };
            let right = match to_number_coerced(ctx, state, *right) {
                Ok(number) => number,
                Err(exception) => return Some(exception),
            };
            value::encode_f64(f64::from(to_int32(left).wrapping_mul(to_int32(right))))
        }
        Builtin::MathHypot => {
            let mut result = 0.0_f64;
            for argument in args {
                let number = match to_number_coerced(ctx, state, *argument) {
                    Ok(number) => number,
                    Err(exception) => return Some(exception),
                };
                result = result.hypot(number);
            }
            value::encode_f64(result)
        }
        Builtin::MathMax => extremum(ctx, state, args, true),
        Builtin::MathMin => extremum(ctx, state, args, false),
        Builtin::MathMaxArray => {
            let Some(array) = args.first().and_then(|array| object_handle(*array)) else {
                return Some(fail_dispatch(ctx));
            };
            let Ok(length) = state.gc.heap().array_length(array) else {
                return Some(fail_dispatch(ctx));
            };
            let mut values = Vec::with_capacity(length as usize);
            for index in 0..length {
                let Ok(Some(element)) = state.gc.heap().get_element(array, index) else {
                    return Some(fail_dispatch(ctx));
                };
                values.push(element as i64);
            }
            extremum(ctx, state, &values, true)
        }
        Builtin::MathRound => match first_number(ctx, state, args) {
            Ok(number) => {
                let rounded = if (-0.5..0.0).contains(&number) {
                    -0.0
                } else {
                    (number + 0.5).floor()
                };
                value::encode_f64(rounded)
            }
            Err(exception) => exception,
        },
        Builtin::MathSign => match first_number(ctx, state, args) {
            Ok(number) => value::encode_f64(if number == 0.0 || number.is_nan() {
                number
            } else {
                number.signum()
            }),
            Err(exception) => exception,
        },
        _ => return None,
    })
}

fn first_number(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> Result<f64, i64> {
    to_number_coerced(
        ctx,
        state,
        args.first()
            .copied()
            .unwrap_or_else(value::encode_undefined),
    )
}

fn unary_number(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    operation: fn(f64) -> f64,
) -> i64 {
    match first_number(ctx, state, args) {
        Ok(number) => value::encode_f64(operation(number)),
        Err(exception) => exception,
    }
}

fn binary_number(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    operation: fn(f64, f64) -> f64,
) -> i64 {
    let [left, right] = args else {
        return fail_dispatch(ctx);
    };
    let left = match to_number_coerced(ctx, state, *left) {
        Ok(number) => number,
        Err(exception) => return exception,
    };
    let right = match to_number_coerced(ctx, state, *right) {
        Ok(number) => number,
        Err(exception) => return exception,
    };
    value::encode_f64(operation(left, right))
}

fn extremum(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    maximum: bool,
) -> i64 {
    let mut result = if maximum {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    };
    for argument in args {
        let number = match to_number_coerced(ctx, state, *argument) {
            Ok(number) => number,
            Err(exception) => return exception,
        };
        if number.is_nan() {
            return value::encode_f64(f64::NAN);
        }
        if maximum {
            if number > result || (number == 0.0 && result == 0.0 && number.is_sign_positive()) {
                result = number;
            }
        } else if number < result || (number == 0.0 && result == 0.0 && number.is_sign_negative()) {
            result = number;
        }
    }
    value::encode_f64(result)
}
