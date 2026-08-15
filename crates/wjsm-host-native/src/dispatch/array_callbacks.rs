use std::cmp::Ordering;

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, is_truthy, render_value, to_number, type_error};
use crate::NativeAgentState;

pub(super) fn dispatch_array_callback(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::ArrayForEach => iterate(ctx, state, args, IterationKind::ForEach),
        Builtin::ArrayMap => iterate(ctx, state, args, IterationKind::Map),
        Builtin::ArrayFilter => iterate(ctx, state, args, IterationKind::Filter),
        Builtin::ArrayFind => iterate(ctx, state, args, IterationKind::Find),
        Builtin::ArrayFindIndex => iterate(ctx, state, args, IterationKind::FindIndex),
        Builtin::ArrayFindLast => iterate(ctx, state, args, IterationKind::FindLast),
        Builtin::ArrayFindLastIndex => iterate(ctx, state, args, IterationKind::FindLastIndex),
        Builtin::ArraySome => iterate(ctx, state, args, IterationKind::Some),
        Builtin::ArrayEvery => iterate(ctx, state, args, IterationKind::Every),
        Builtin::ArrayFlatMap => iterate(ctx, state, args, IterationKind::FlatMap),
        Builtin::ArrayReduce => reduce(ctx, state, args, false),
        Builtin::ArrayReduceRight => reduce(ctx, state, args, true),
        Builtin::ArraySort => sort(ctx, state, args, false),
        Builtin::ArrayToSorted => sort(ctx, state, args, true),
        _ => return None,
    })
}

fn array(args: &[i64]) -> Option<(i64, u32)> {
    let encoded = *args.first()?;
    value::is_array(encoded).then(|| (encoded, value::decode_handle(encoded)))
}

fn length(state: &NativeAgentState, handle: u32) -> Option<u32> {
    state.heap.array_length(handle).ok()
}

fn raw(state: &NativeAgentState, handle: u32, index: u32) -> Option<i64> {
    state
        .heap
        .get_element(handle, index)
        .ok()
        .flatten()
        .map(|value| value as i64)
}

fn observable(raw: Option<i64>) -> i64 {
    raw.filter(|value| !value::is_array_hole(*value))
        .unwrap_or_else(value::encode_undefined)
}

fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callback: i64,
    this_value: i64,
    args: &[i64],
) -> Result<i64, i64> {
    let result = state
        .invoke_callable(ctx, callback, this_value, args)
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        Err(result)
    } else {
        Ok(result)
    }
}

#[derive(Clone, Copy)]
enum IterationKind {
    Every,
    Filter,
    Find,
    FindIndex,
    FindLast,
    FindLastIndex,
    FlatMap,
    ForEach,
    Map,
    Some,
}

fn iterate(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: IterationKind,
) -> i64 {
    let Some((array_value, handle)) = array(args) else {
        return fail_dispatch(ctx);
    };
    let Some(callback) = args
        .get(1)
        .copied()
        .filter(|value| value::is_callable(*value))
    else {
        return fail_dispatch(ctx);
    };
    let this_value = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    let Some(array_length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let result_array = match kind {
        IterationKind::Map => {
            let Ok(array) = state.allocate_object(array_length, true) else {
                return fail_dispatch(ctx);
            };
            if state
                .heap
                .set_array_length(value::decode_handle(array), array_length)
                .is_err()
            {
                return fail_dispatch(ctx);
            }
            Some(array)
        }
        IterationKind::Filter | IterationKind::FlatMap => {
            match state.allocate_object(array_length, true) {
                Ok(array) => Some(array),
                Err(_) => return fail_dispatch(ctx),
            }
        }
        _ => None,
    };
    let visits_holes = matches!(
        kind,
        IterationKind::Find
            | IterationKind::FindIndex
            | IterationKind::FindLast
            | IterationKind::FindLastIndex
    );
    let reverse = matches!(kind, IterationKind::FindLast | IterationKind::FindLastIndex);
    let indices: Box<dyn Iterator<Item = u32>> = if reverse {
        Box::new((0..array_length).rev())
    } else {
        Box::new(0..array_length)
    };
    for index in indices {
        let element_raw = raw(state, handle, index);
        if !visits_holes && element_raw.is_none_or(value::is_array_hole) {
            continue;
        }
        let element = observable(element_raw);
        let callback_args = [element, value::encode_f64(f64::from(index)), array_value];
        let callback_result = match call(ctx, state, callback, this_value, &callback_args) {
            Ok(result) => result,
            Err(exception) => return exception,
        };
        match kind {
            IterationKind::ForEach => {}
            IterationKind::Map => {
                let output = value::decode_handle(result_array.expect("map allocates output"));
                if state
                    .heap
                    .set_element(output, index, callback_result as u64)
                    .is_err()
                {
                    return fail_dispatch(ctx);
                }
            }
            IterationKind::Filter => {
                if is_truthy(state, callback_result) {
                    let output =
                        value::decode_handle(result_array.expect("filter allocates output"));
                    if state.heap.push_element(output, element as u64).is_err() {
                        return fail_dispatch(ctx);
                    }
                }
            }
            IterationKind::FlatMap => {
                let output = value::decode_handle(result_array.expect("flatMap allocates output"));
                if value::is_array(callback_result) {
                    let inner = value::decode_handle(callback_result);
                    let Some(inner_length) = length(state, inner) else {
                        return fail_dispatch(ctx);
                    };
                    for inner_index in 0..inner_length {
                        let inner_value = observable(raw(state, inner, inner_index));
                        if state.heap.push_element(output, inner_value as u64).is_err() {
                            return fail_dispatch(ctx);
                        }
                    }
                } else if state
                    .heap
                    .push_element(output, callback_result as u64)
                    .is_err()
                {
                    return fail_dispatch(ctx);
                }
            }
            IterationKind::Find if is_truthy(state, callback_result) => return element,
            IterationKind::FindIndex if is_truthy(state, callback_result) => {
                return value::encode_f64(f64::from(index));
            }
            IterationKind::FindLast if is_truthy(state, callback_result) => return element,
            IterationKind::FindLastIndex if is_truthy(state, callback_result) => {
                return value::encode_f64(f64::from(index));
            }
            IterationKind::Some if is_truthy(state, callback_result) => {
                return value::encode_bool(true);
            }
            IterationKind::Every if !is_truthy(state, callback_result) => {
                return value::encode_bool(false);
            }
            IterationKind::Find
            | IterationKind::FindIndex
            | IterationKind::FindLast
            | IterationKind::FindLastIndex
            | IterationKind::Some
            | IterationKind::Every => {}
        }
    }
    match kind {
        IterationKind::Map | IterationKind::Filter | IterationKind::FlatMap => {
            result_array.expect("array-producing iteration allocates output")
        }
        IterationKind::Find | IterationKind::FindLast => value::encode_undefined(),
        IterationKind::FindIndex | IterationKind::FindLastIndex => value::encode_f64(-1.0),
        IterationKind::Some => value::encode_bool(false),
        IterationKind::Every => value::encode_bool(true),
        IterationKind::ForEach => value::encode_undefined(),
    }
}

fn reduce(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    reverse: bool,
) -> i64 {
    let Some((array_value, handle)) = array(args) else {
        return fail_dispatch(ctx);
    };
    let Some(callback) = args
        .get(1)
        .copied()
        .filter(|value| value::is_callable(*value))
    else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let indices: Vec<u32> = if reverse {
        (0..length).rev().collect()
    } else {
        (0..length).collect()
    };
    let mut position = 0;
    let mut accumulator = if let Some(initial) = args.get(2).copied() {
        initial
    } else {
        loop {
            let Some(index) = indices.get(position).copied() else {
                return type_error(ctx, state, "Reduce of empty array with no initial value");
            };
            position += 1;
            let Some(element) =
                raw(state, handle, index).filter(|element| !value::is_array_hole(*element))
            else {
                continue;
            };
            break element;
        }
    };
    for index in indices.into_iter().skip(position) {
        let Some(element) =
            raw(state, handle, index).filter(|element| !value::is_array_hole(*element))
        else {
            continue;
        };
        let callback_args = [
            accumulator,
            element,
            value::encode_f64(f64::from(index)),
            array_value,
        ];
        accumulator = match call(
            ctx,
            state,
            callback,
            value::encode_undefined(),
            &callback_args,
        ) {
            Ok(result) => result,
            Err(exception) => return exception,
        };
    }
    accumulator
}

fn sort(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64], copy: bool) -> i64 {
    let Some((array_value, handle)) = array(args) else {
        return fail_dispatch(ctx);
    };
    let comparator = args
        .get(1)
        .copied()
        .filter(|value| !value::is_undefined(*value));
    if comparator.is_some_and(|value| !value::is_callable(value)) {
        return fail_dispatch(ctx);
    }
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let mut values: Vec<_> = if copy {
        (0..length)
            .map(|index| observable(raw(state, handle, index)))
            .collect()
    } else {
        (0..length)
            .filter_map(|index| raw(state, handle, index))
            .filter(|stored| !value::is_array_hole(*stored))
            .collect()
    };
    let sorted = super::array_sort::stable_sort_by(&mut values, |left, right| {
        compare(ctx, state, comparator, left, right)
    });
    if let Err(exception) = sorted {
        return exception;
    }
    if copy {
        state
            .allocate_array_values(&values)
            .unwrap_or_else(|_| fail_dispatch(ctx))
    } else {
        let present = values.len() as u32;
        for (index, stored) in values.into_iter().enumerate() {
            if state
                .heap
                .set_element(handle, index as u32, stored as u64)
                .is_err()
            {
                return fail_dispatch(ctx);
            }
        }
        for index in present..length {
            if state
                .heap
                .set_element(handle, index, value::encode_array_hole() as u64)
                .is_err()
            {
                return fail_dispatch(ctx);
            }
        }
        array_value
    }
}

fn compare(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    comparator: Option<i64>,
    left: i64,
    right: i64,
) -> Result<Ordering, i64> {
    if value::is_undefined(left) || value::is_undefined(right) {
        return Ok(
            match (value::is_undefined(left), value::is_undefined(right)) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => unreachable!(),
            },
        );
    }
    if let Some(comparator) = comparator {
        let result = call(
            ctx,
            state,
            comparator,
            value::encode_undefined(),
            &[left, right],
        )?;
        let number = to_number(state, result).ok_or_else(|| fail_dispatch(ctx))?;
        Ok(if number.is_nan() || number == 0.0 {
            Ordering::Equal
        } else if number < 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        })
    } else {
        Ok(render_value(state, left).cmp(&render_value(state, right)))
    }
}
