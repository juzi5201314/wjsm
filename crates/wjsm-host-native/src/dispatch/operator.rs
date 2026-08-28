//! 运算符 / 反射类 builtin 的宿主实现：比较、typeof、instanceof、in、
//! 异常值桥、new.target、debugger 等零散语义。

use std::cmp::Ordering;

use num_bigint::BigInt;
use num_traits::FromPrimitive;

use wjsm_ir::{Builtin, value, wk_symbol};
use wjsm_native_abi::NativeVmContext;

use super::bigint;
use super::proxy;
use super::runtime::PrimitiveHint;
use super::runtime::{
    abstract_equal, fail_dispatch, get_property, has_property, is_truthy, reference_error,
    render_value, strict_equal, to_number, to_primitive, type_error,
};
use crate::{NativeAgentState, NativeCallableKind};

pub(super) fn dispatch_operator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::AbstractCompare => abstract_compare_builtin(ctx, state, args),
        Builtin::AbstractEq => {
            let [left, right] = args else {
                return Some(fail_dispatch(ctx));
            };
            // 操作数求值异常（lval 先于 rval）已由 dispatch_builtin 入口的
            // 实参哨兵透传统一处理，此处只做比较本身。
            match abstract_equal(ctx, state, *left, *right) {
                Ok(equal) => value::encode_bool(equal),
                Err(exception) => exception,
            }
        }
        Builtin::StrictEq => {
            let [left, right] = args else {
                return Some(fail_dispatch(ctx));
            };
            // 严格相等自身不抛；操作数哨兵已在 dispatch 入口透传。
            value::encode_bool(strict_equal(state, *left, *right))
        }
        Builtin::TypeOf => type_of(ctx, state, args),
        Builtin::InstanceOf => instance_of(ctx, state, args),
        Builtin::In => {
            let [object, key] = args else {
                return Some(fail_dispatch(ctx));
            };
            op_in(ctx, state, *object, *key)
        }
        Builtin::Throw => args
            .first()
            .and_then(|argument| state.create_exception(*argument))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::ExceptionValue => args
            .first()
            .and_then(|exception| state.exception_value(*exception))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::NewTarget => state
            .activations
            .last()
            .map(|activation| activation.new_target)
            .unwrap_or_else(value::encode_undefined),
        Builtin::Debugger => {
            crate::inspector::pause(ctx, state, "debuggerStatement");
            value::encode_undefined()
        }
        Builtin::IsCallable => args
            .first()
            .map(|input| value::encode_bool(value::is_callable(*input)))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::IsJsObject => args
            .first()
            .map(|input| value::encode_bool(value::is_js_object(*input)))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::IsString => args
            .first()
            .map(|input| value::encode_bool(value::is_runtime_string_handle(*input)))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::TdzCheck => {
            let [checked, name] = args else {
                return Some(fail_dispatch(ctx));
            };
            if value::is_uninitialized(*checked) {
                let name = state.string_to_utf8(*name).unwrap_or_default();
                reference_error(
                    ctx,
                    state,
                    &format!("Cannot access '{name}' before initialization"),
                )
            } else {
                *checked
            }
        }
        Builtin::ToPropertyKey => {
            let [key] = args else {
                return Some(fail_dispatch(ctx));
            };
            match super::runtime::to_property_key_value(ctx, state, *key) {
                Ok(key) => key,
                Err(exception) => exception,
            }
        }
        Builtin::GetPrototypeFromConstructor => {
            let Some(constructor) = args.first().copied() else {
                return Some(fail_dispatch(ctx));
            };
            let Some(prototype_key) = state.intern_property_string("prototype".into()) else {
                return Some(fail_dispatch(ctx));
            };
            state
                .callable_property(constructor, prototype_key)
                .filter(|prototype| value::is_js_object(*prototype))
                .unwrap_or_else(value::encode_null)
        }
        _ => return None,
    })
}

/// 抽象关系比较：`reverse` 交换左右操作数，`invert` 反转结果（`>` / `>=`）。
fn abstract_compare_builtin(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [left, right, reverse, invert] = args else {
        return fail_dispatch(ctx);
    };
    let left = match to_primitive(ctx, state, *left, PrimitiveHint::Number) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let right = match to_primitive(ctx, state, *right, PrimitiveHint::Number) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let comparison = if value::decode_bool(*reverse) {
        abstract_compare(state, right, left)
    } else {
        abstract_compare(state, left, right)
    };
    let result = if value::decode_bool(*invert) {
        comparison.is_some_and(|ordering| ordering != Ordering::Less)
    } else {
        comparison == Some(Ordering::Less)
    };
    value::encode_bool(result)
}

fn type_of(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(input) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let name = if value::is_undefined(input) {
        "undefined"
    } else if value::is_bool(input) {
        "boolean"
    } else if value::is_string(input) {
        "string"
    } else if value::is_callable(input) {
        "function"
    } else if value::is_bigint(input) {
        "bigint"
    } else if value::is_symbol(input) {
        "symbol"
    } else if value::is_null(input) || value::is_js_object(input) || value::is_regexp(input) {
        "object"
    } else {
        "number"
    };
    state
        .intern_text(name.into(), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

/// ES §13.10.1 `in`：lval（key）先于 rval（object）求值。async 状态机等
/// 不插表达式级分叉的上下文里，求值异常以 TAG_EXCEPTION 流入，按求值顺序
/// 原样透传；rval 非对象（null/undefined/原始值）抛 TypeError（步骤 5，
/// 先于 ToPropertyKey），文案与 V8/Node 对齐；Proxy 走 has trap，trap
/// 异常由返回值传播。
fn op_in(ctx: &mut NativeVmContext, state: &mut NativeAgentState, object: i64, key: i64) -> i64 {
    if value::is_exception(key) {
        return key;
    }
    if value::is_exception(object) {
        return object;
    }
    if !(value::is_js_object(object) || value::is_regexp(object)) {
        let rendered_key = render_value(state, key);
        let rendered_object = render_value(state, object);
        return type_error(
            ctx,
            state,
            &format!(
                "Cannot use 'in' operator to search for '{rendered_key}' in {rendered_object}"
            ),
        );
    }
    if value::is_proxy(object) {
        return proxy::has(ctx, state, &[object, key]);
    }
    value::encode_bool(has_property(state, object, key))
}

/// ES InstanceofOperator：步骤 1 target 非对象抛 TypeError（先于
/// @@hasInstance 查找），随后 @@hasInstance → OrdinaryHasInstance。
/// 操作数求值异常（lval 先于 rval，与实参序一致）已由 dispatch_builtin
/// 入口的实参哨兵透传统一处理。
fn instance_of(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [object, constructor] = args else {
        return fail_dispatch(ctx);
    };
    if !(value::is_js_object(*constructor) || value::is_regexp(*constructor)) {
        return type_error(
            ctx,
            state,
            "Right-hand side of 'instanceof' is not an object",
        );
    }
    let has_instance_key = value::encode_handle(value::TAG_SYMBOL, wk_symbol::HAS_INSTANCE);
    let method = match get_property(ctx, state, *constructor, has_instance_key) {
        Ok(method) => method,
        // 步骤 1 已保证 target 是对象，取 @@hasInstance 失败属内部错误。
        Err(()) => return fail_dispatch(ctx),
    };
    if value::is_exception(method) {
        return method;
    }
    if !value::is_undefined(method) {
        if !value::is_callable(method) {
            return type_error(ctx, state, "Symbol.hasInstance method is not callable");
        }
        let result = state
            .invoke_callable(ctx, method, *constructor, &[*object])
            .unwrap_or_else(|| fail_dispatch(ctx));
        if value::is_exception(result) {
            return result;
        }
        return value::encode_bool(is_truthy(state, result));
    }
    if !state.is_callable_value(*constructor) {
        return type_error(
            ctx,
            state,
            "Right-hand side of 'instanceof' is not callable",
        );
    }
    // OrdinaryHasInstance 步骤 2：bound function（Function.prototype.bind 产物，
    // 表示为 NativeCallableKind::Bound）委托 [[BoundTargetFunction]] 重新走
    // InstanceofOperator（含目标自身的 @@hasInstance 查找），而不是读 bound
    // 包装（无 prototype 属性）误抛 TypeError。
    if let Some(NativeCallableKind::Bound(index)) = state.native_callable_kind(*constructor) {
        let Some(target) = state
            .bound_functions
            .get(index as usize)
            .and_then(|bound| bound.as_ref())
            .map(|bound| bound.target)
        else {
            return fail_dispatch(ctx);
        };
        return instance_of(ctx, state, &[*object, target]);
    }
    let Some(prototype_key) = state.intern_text("prototype".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let prototype = match get_property(ctx, state, *constructor, prototype_key) {
        Ok(prototype) => prototype,
        Err(()) => return fail_dispatch(ctx),
    };
    if value::is_exception(prototype) {
        return prototype;
    }
    // prototype 只需是对象（含 RegExp 等 exotic 对象）；非对象时文案与
    // V8/Node 对齐（渲染实际值）。
    if !(value::is_object(prototype)
        || value::is_array(prototype)
        || value::is_callable(prototype)
        || value::is_proxy(prototype)
        || value::is_regexp(prototype))
    {
        let rendered = render_value(state, prototype);
        return type_error(
            ctx,
            state,
            &format!("Function has non-object prototype '{rendered}' in instanceof check"),
        );
    }
    // 左操作数是可调用值时，其默认原型链经 %Function.prototype% →
    // %Object.prototype%（见 prototype_chain_contains_value）；两个 intrinsic
    // 都是惰性分配，先确保存在，避免链遍历因尚未分配而漏判。
    if value::is_callable(*object)
        && (state
            .native_callable(NativeCallableKind::FunctionPrototype)
            .is_none()
            || state.ensure_intrinsic_prototypes().is_err())
    {
        return fail_dispatch(ctx);
    }
    value::encode_bool(state.prototype_chain_contains_value(*object, prototype))
}

fn abstract_compare(state: &NativeAgentState, left: i64, right: i64) -> Option<Ordering> {
    if value::is_string(left) && value::is_string(right) {
        let left = state.with_string_units(left, |units| units.to_vec())?;
        let right = state.with_string_units(right, |units| units.to_vec())?;
        return Some(left.cmp(&right));
    }
    match (value::is_bigint(left), value::is_bigint(right)) {
        (true, true) => bigint_compare(state, left, right),
        (true, false) => {
            bigint_number_compare(&bigint::read(state, left)?, to_number(state, right)?)
        }
        (false, true) => {
            bigint_number_compare(&bigint::read(state, right)?, to_number(state, left)?)
                .map(Ordering::reverse)
        }
        (false, false) => to_number(state, left)?.partial_cmp(&to_number(state, right)?),
    }
}

fn bigint_compare(state: &NativeAgentState, left: i64, right: i64) -> Option<Ordering> {
    bigint::read(state, left)?.partial_cmp(&bigint::read(state, right)?)
}

fn bigint_number_compare(bigint: &BigInt, number: f64) -> Option<Ordering> {
    if number.is_nan() {
        return None;
    }
    if number == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if number == f64::NEG_INFINITY {
        return Some(Ordering::Greater);
    }
    let integral = BigInt::from_f64(number.trunc())?;
    let comparison = bigint.cmp(&integral);
    if comparison != Ordering::Equal || number.fract() == 0.0 {
        return Some(comparison);
    }
    Some(if number.is_sign_positive() {
        Ordering::Less
    } else {
        Ordering::Greater
    })
}
