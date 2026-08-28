use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{
    fail_dispatch, get_property, to_number_coerced, to_string_coerced, type_error,
};
use crate::{FUNCTION_METADATA_FLAGS, NativeAgentState, NativeBoundFunction, NativeCallableKind};

pub(super) fn dispatch_function(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::FuncBind => {
            let Some((&target, rest)) = args.split_first() else {
                return Some(fail_dispatch(ctx));
            };
            if !value::is_callable(target) {
                return Some(type_error(
                    ctx,
                    state,
                    "Function.prototype.bind target is not callable",
                ));
            }
            let (this_value, arguments) = rest.split_first().map_or(
                (value::encode_undefined(), &[][..]),
                |(this_value, arguments)| (*this_value, arguments),
            );
            let index = match state.bound_free.pop() {
                Some(index) => index,
                None => {
                    let Ok(index) = u32::try_from(state.bound_functions.len()) else {
                        return Some(fail_dispatch(ctx));
                    };
                    state.bound_functions.push(None);
                    index
                }
            };
            state.bound_functions[index as usize] = Some(NativeBoundFunction {
                target,
                this_value,
                arguments: arguments.to_vec(),
            });
            let Some(bound) = state.native_callable(NativeCallableKind::Bound(index)) else {
                return Some(fail_dispatch(ctx));
            };
            state.gc.record_host_write(bound, None, Some(bound));
            for stored in std::iter::once(target)
                .chain(std::iter::once(this_value))
                .chain(arguments.iter().copied())
            {
                state.gc.record_host_write(bound, None, Some(stored));
            }
            if let Err(exception) =
                initialize_bound_metadata(ctx, state, bound, target, arguments.len())
            {
                return Some(exception);
            }
            bound
        }
        Builtin::FuncCall => {
            let Some((&callee, rest)) = args.split_first() else {
                return Some(fail_dispatch(ctx));
            };
            let (this_value, arguments) = rest.split_first().map_or(
                (value::encode_undefined(), &[][..]),
                |(this_value, arguments)| (*this_value, arguments),
            );
            state
                .invoke_callable(ctx, callee, this_value, arguments)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::FuncApply => {
            let Some(callee) = args.first().copied() else {
                return Some(fail_dispatch(ctx));
            };
            let this_value = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            let argument_list = args.get(2).copied().unwrap_or_else(value::encode_undefined);
            let arguments = match create_list_from_array_like(ctx, state, argument_list) {
                Ok(arguments) => arguments,
                Err(exception) => return Some(exception),
            };
            state
                .invoke_callable(ctx, callee, this_value, &arguments)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::SuperApply => {
            // spread 形态的 SuperCall（ES §13.3.7.1 步骤 3）：
            // Construct(func, argList, GetNewTarget())——是 [[Construct]] 而非
            // [[Call]]，newTarget 沿用当前（派生构造器）activation 的值，与
            // prepare_super_call 的非 spread 路径一致；否则类构造器基类会被
            // [[Call]] 门禁误拒，new.target 也会丢失。
            let Some(callee) = args.first().copied() else {
                return Some(fail_dispatch(ctx));
            };
            let this_value = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            let argument_list = args.get(2).copied().unwrap_or_else(value::encode_undefined);
            let arguments = match create_list_from_array_like(ctx, state, argument_list) {
                Ok(arguments) => arguments,
                Err(exception) => return Some(exception),
            };
            let new_target = state
                .activations
                .last()
                .map_or_else(value::encode_undefined, |activation| activation.new_target);
            state
                .invoke_constructor(ctx, callee, new_target, this_value, &arguments)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::CreateClosure => state
            .create_closure(args)
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::FunctionSetName => {
            let [function, key, prefix] = args else {
                return Some(fail_dispatch(ctx));
            };
            set_runtime_function_name(ctx, state, *function, *key, *prefix)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::FunctionToString => {
            let Some(&receiver) = args.first() else {
                return Some(fail_dispatch(ctx));
            };
            function_to_string(ctx, state, receiver)
        }
        _ => return None,
    })
}

/// `Function.prototype.toString`（ES §20.2.3.5）：this 非 callable 抛 TypeError
/// （步骤 5，文案对齐 V8）；其余按 [[SourceText]] / NativeFunction 形态返回。
fn function_to_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
) -> i64 {
    let receiver = value::strip_gc_color(receiver);
    if !state.is_callable_value(receiver) {
        return type_error(
            ctx,
            state,
            "Function.prototype.toString requires that 'this' be a Function",
        );
    }
    let Some(text) = state.callable_to_string_source(receiver) else {
        return fail_dispatch(ctx);
    };
    state
        .intern_text(text, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

/// SetFunctionName（ES §10.2.9）的运行时形态：计算键在 ToPropertyKey 之后才
/// 确定，语义层在方法/访问器/匿名函数定义点发射本调用。symbol 键取
/// `[description]`（无 description 为空串）；prefix 编码 0/1/2 对应
/// 无前缀 / `get ` / `set `。
fn set_runtime_function_name(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    function: i64,
    key: i64,
    prefix: i64,
) -> Option<i64> {
    let function = value::strip_gc_color(function);
    if !value::is_callable(function) {
        return None;
    }
    let prefix = if value::is_f64(prefix) {
        value::decode_f64(prefix) as u32
    } else {
        0
    };
    let mut units: Vec<u16> = match prefix {
        1 => "get ".encode_utf16().collect(),
        2 => "set ".encode_utf16().collect(),
        _ => Vec::new(),
    };
    if value::is_symbol(key) {
        if let Some(description) = state.symbol_description(key) {
            units.push(u16::from(b'['));
            units.extend_from_slice(description.as_flat_slice());
            units.push(u16::from(b']'));
        }
    } else {
        // ToPropertyKey 之后的非 symbol 键必为 primitive，ToString 不会再入用户代码。
        let text = to_string_coerced(ctx, state, key).ok()?;
        units.extend(text.encode_utf16());
    }
    let name_value = state.intern_utf16_slice(&units, value::TAG_STRING)?;
    let name_key = state.intern_property_string("name".into())?;
    state
        .callable_properties
        .insert((function, name_key), name_value);
    state
        .callable_property_flags
        .insert((function, name_key), FUNCTION_METADATA_FLAGS);
    Some(value::encode_undefined())
}

/// BoundFunctionCreate 之后按 Function.prototype.bind（ES §20.2.3.2 步骤
/// 2–6）初始化 bound 函数的 `length` / `name`：length 为
/// `max(0, ToIntegerOrInfinity(Get(target,"length")) - 已绑实参数)`（目标
/// 自有 length 非 number 时取 0），name 为 `"bound " + Get(target,"name")`
/// （非 string 取空串）。Get 可经 Proxy 再入用户代码，异常原样传播。
fn initialize_bound_metadata(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    bound: i64,
    target: i64,
    bound_argument_count: usize,
) -> Result<(), i64> {
    let bound = value::strip_gc_color(bound);
    let length_text = state
        .intern_text("length".into(), value::TAG_STRING)
        .ok_or_else(|| fail_dispatch(ctx))?;
    let target_length =
        get_property(ctx, state, target, length_text).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(target_length) {
        return Err(target_length);
    }
    let length = if value::is_f64(target_length) {
        // ToIntegerOrInfinity：NaN → 0，±∞ 保留，其余向零取整。
        let target_length = value::decode_f64(target_length);
        let target_length = if target_length.is_nan() {
            0.0
        } else {
            target_length.trunc()
        };
        (target_length - bound_argument_count as f64).max(0.0)
    } else {
        0.0
    };
    let name_text = state
        .intern_text("name".into(), value::TAG_STRING)
        .ok_or_else(|| fail_dispatch(ctx))?;
    let target_name =
        get_property(ctx, state, target, name_text).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(target_name) {
        return Err(target_name);
    }
    let mut units: Vec<u16> = "bound ".encode_utf16().collect();
    if let Some(name) = state.string_owned(target_name) {
        units.extend_from_slice(name.as_flat_slice());
    }
    let name_value = state
        .intern_utf16_slice(&units, value::TAG_STRING)
        .ok_or_else(|| fail_dispatch(ctx))?;
    let length_key = state
        .intern_property_string("length".into())
        .ok_or_else(|| fail_dispatch(ctx))?;
    let name_key = state
        .intern_property_string("name".into())
        .ok_or_else(|| fail_dispatch(ctx))?;
    state
        .callable_properties
        .insert((bound, length_key), value::encode_f64(length));
    state
        .callable_property_flags
        .insert((bound, length_key), FUNCTION_METADATA_FLAGS);
    state
        .callable_properties
        .insert((bound, name_key), name_value);
    state
        .callable_property_flags
        .insert((bound, name_key), FUNCTION_METADATA_FLAGS);
    Ok(())
}

fn create_list_from_array_like(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    source: i64,
) -> Result<Vec<i64>, i64> {
    if value::is_null(source) || value::is_undefined(source) {
        return Ok(Vec::new());
    }
    if !value::is_js_object(source) && !value::is_regexp(source) {
        return Err(type_error(
            ctx,
            state,
            "Function.prototype.apply expects an array-like object",
        ));
    }
    if value::is_array(source) {
        let handle = value::decode_handle(source);
        let length = state
            .gc
            .heap()
            .array_length(handle)
            .map_err(|_| fail_dispatch(ctx))?;
        return (0..length)
            .map(|index| {
                state
                    .gc
                    .heap()
                    .get_element(handle, index)
                    .map(|stored| {
                        stored
                            .filter(|stored| !value::is_array_hole(*stored as i64))
                            .map_or_else(value::encode_undefined, |stored| stored as i64)
                    })
                    .map_err(|_| fail_dispatch(ctx))
            })
            .collect();
    }

    let length_key = state
        .intern_text("length".into(), value::TAG_STRING)
        .ok_or_else(|| fail_dispatch(ctx))?;
    let encoded_length =
        get_property(ctx, state, source, length_key).map_err(|_| fail_dispatch(ctx))?;
    if value::is_exception(encoded_length) {
        return Err(encoded_length);
    }
    let number = to_number_coerced(ctx, state, encoded_length)?;
    let length = if !number.is_finite() || number <= 0.0 {
        0
    } else {
        number.trunc().min(f64::from(u32::MAX)) as u32
    };
    let mut arguments = Vec::with_capacity(length as usize);
    for index in 0..length {
        let key = state
            .intern_text(index.to_string(), value::TAG_STRING)
            .ok_or_else(|| fail_dispatch(ctx))?;
        let argument = get_property(ctx, state, source, key).map_err(|_| fail_dispatch(ctx))?;
        if value::is_exception(argument) {
            return Err(argument);
        }
        arguments.push(argument);
    }
    Ok(arguments)
}
