use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, get_property, to_number_coerced, type_error};
use crate::{NativeAgentState, NativeBoundFunction, NativeCallableKind};

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
            let Ok(index) = u32::try_from(state.bound_functions.len()) else {
                return Some(fail_dispatch(ctx));
            };
            state.bound_functions.push(NativeBoundFunction {
                target,
                this_value,
                arguments: arguments.to_vec(),
            });
            state
                .native_callable(NativeCallableKind::Bound(index))
                .unwrap_or_else(|| fail_dispatch(ctx))
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
        Builtin::FuncApply | Builtin::SuperApply => {
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
        _ => return None,
    })
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
            .heap
            .array_length(handle)
            .map_err(|_| fail_dispatch(ctx))?;
        return (0..length)
            .map(|index| {
                state
                    .heap
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
