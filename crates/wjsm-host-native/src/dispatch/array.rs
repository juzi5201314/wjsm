use wjsm_host::RuntimeString;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{
    allocate_object_or_out_of_memory, array_index, fail_dispatch, get_property, has_property,
    is_truthy, iterator_done, iterator_value, strict_equal, to_number, to_string_coerced,
};
use crate::NativeAgentState;

pub(super) fn dispatch_array(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::ArrayPush => array_push(ctx, state, args),
        Builtin::ArrayPushHole => array_push_hole(ctx, state, args),
        Builtin::ArrayPushSpread => array_push_spread(ctx, state, args),
        Builtin::ArrayPop => array_pop(ctx, state, args),
        Builtin::ArraySpliceVa => array_splice(ctx, state, args),
        Builtin::ArrayIncludes => array_search(ctx, state, args, SearchKind::Includes),
        Builtin::ArrayIndexOf => array_search(ctx, state, args, SearchKind::IndexOf),
        Builtin::ArrayLastIndexOf => array_search(ctx, state, args, SearchKind::LastIndexOf),
        Builtin::ArrayJoin => array_join(ctx, state, args),
        Builtin::ArrayConcat | Builtin::ArrayConcatVa => array_concat(ctx, state, args),
        Builtin::ArraySlice => array_slice(ctx, state, args),
        Builtin::ArrayFill => array_fill(ctx, state, args),
        Builtin::ArrayFlat => array_flat(ctx, state, args),
        Builtin::ArrayReverse => array_reverse(ctx, state, args),
        Builtin::ArrayInitLength => array_set_length(ctx, state, args),
        Builtin::ArrayGetLength => array_length(ctx, state, args),
        Builtin::ArrayShift => array_shift(ctx, state, args),
        Builtin::ArrayUnshiftVa => array_unshift(ctx, state, args),
        Builtin::ArrayAt => array_at(ctx, state, args),
        Builtin::ArrayCopyWithin => array_copy_within(ctx, state, args),
        Builtin::ArrayIsArray => {
            value::encode_bool(args.first().is_some_and(|value| value::is_array(*value)))
        }
        Builtin::ArrayAllocate => array_allocate(ctx, state, args),
        Builtin::ArrayHasElement => array_has_element(ctx, state, args),
        Builtin::ArrayFrom => array_from(ctx, state, args),
        Builtin::ArrayOf => state
            .allocate_array_values(args)
            .unwrap_or_else(|_| fail_dispatch(ctx)),
        Builtin::ArrayToReversed => array_to_reversed(ctx, state, args),
        Builtin::ArrayWith => array_with(ctx, state, args),
        Builtin::ArrayToSplicedVa => array_to_spliced(ctx, state, args),
        _ => return None,
    })
}
pub(crate) fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    if let [length] = args
        && value::is_f64(*length)
    {
        let length = value::decode_f64(*length);
        if !length.is_finite()
            || length < 0.0
            || length > f64::from(u32::MAX)
            || length.fract() != 0.0
        {
            return super::runtime::range_error(ctx, state, "Invalid array length");
        }
        let length = length as u32;
        let array = allocate_object_or_out_of_memory(ctx, state, length, true);
        if value::is_exception(array) {
            return array;
        }
        let handle = value::decode_handle(array);
        for index in 0..length {
            if state
                .heap
                .set_element(handle, index, value::encode_array_hole() as u64)
                .is_err()
            {
                return fail_dispatch(ctx);
            }
        }
        if length != 0
            && state
                .heap
                .raise_array_kind(handle, wjsm_ir::constants::ARRAY_KIND_HOLEY)
                .is_err()
        {
            return fail_dispatch(ctx);
        }
        return array;
    }
    state
        .allocate_array_values(args)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn array_from(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(source) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    if value::is_null(source) || value::is_undefined(source) {
        return super::runtime::type_error(ctx, state, "Array.from requires an object");
    }
    let map = args
        .get(1)
        .copied()
        .filter(|map| !value::is_undefined(*map));
    if map.is_some_and(|map| !value::is_callable(map)) {
        return super::runtime::type_error(ctx, state, "Array.from map function is not callable");
    }
    let this_value = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    let iterator_key = value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::ITERATOR);
    let method = match get_property(ctx, state, source, iterator_key) {
        Ok(method) if value::is_exception(method) => return method,
        Ok(method) => method,
        Err(()) => return fail_dispatch(ctx),
    };
    let values = if value::is_undefined(method) || value::is_null(method) {
        let length = match array_like_length(ctx, state, source) {
            Ok(length) => length,
            Err(exception) => return exception,
        };
        let mut values = Vec::with_capacity(length as usize);
        for index in 0..length {
            let key = match state.intern_text(index.to_string(), value::TAG_STRING) {
                Some(key) => key,
                None => return fail_dispatch(ctx),
            };
            let mut stored = match get_property(ctx, state, source, key) {
                Ok(stored) => stored,
                Err(()) => return fail_dispatch(ctx),
            };
            if value::is_exception(stored) {
                return stored;
            }
            if let Some(map) = map {
                stored = state
                    .invoke_callable(
                        ctx,
                        map,
                        this_value,
                        &[stored, value::encode_f64(index as f64)],
                    )
                    .unwrap_or_else(|| fail_dispatch(ctx));
                if value::is_exception(stored) {
                    return stored;
                }
            }
            values.push(stored);
        }
        values
    } else {
        let iterator = super::runtime::iterator_from_method(ctx, state, source, method);
        if value::is_exception(iterator) {
            return iterator;
        }
        let mut values = Vec::new();
        loop {
            let done = iterator_done(ctx, state, &[iterator]);
            if value::is_exception(done) {
                return done;
            }
            if is_truthy(state, done) {
                break;
            }
            let mut stored = iterator_value(ctx, state, &[iterator], true);
            if value::is_exception(stored) {
                return stored;
            }
            if let Some(map) = map {
                stored = state
                    .invoke_callable(
                        ctx,
                        map,
                        this_value,
                        &[stored, value::encode_f64(values.len() as f64)],
                    )
                    .unwrap_or_else(|| fail_dispatch(ctx));
                if value::is_exception(stored) {
                    return stored;
                }
            }
            values.push(stored);
        }
        values
    };
    state
        .allocate_array_values(&values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn handle(args: &[i64]) -> Option<u32> {
    let array = *args.first()?;
    value::is_array(array).then(|| value::decode_handle(array))
}

fn length(state: &NativeAgentState, handle: u32) -> Option<u32> {
    state.heap.array_length(handle).ok()
}

fn get_raw(state: &NativeAgentState, handle: u32, index: u32) -> Option<i64> {
    state
        .heap
        .get_element(handle, index)
        .ok()
        .flatten()
        .map(|value| value as i64)
}

fn get(state: &NativeAgentState, handle: u32, index: u32) -> i64 {
    get_raw(state, handle, index)
        .filter(|value| !value::is_array_hole(*value))
        .unwrap_or_else(value::encode_undefined)
}

fn set(state: &NativeAgentState, handle: u32, index: u32, stored: i64) -> bool {
    state.heap.set_element(handle, index, stored as u64).is_ok()
}

/// `array.allocate(len)`：按长度创建全 hole 数组（length=len），供 map 结果容器。
/// 显式填洞：新分配数组的元素槽为 0（解码为 +0.0），不填洞会让 map 结果把
/// 未写入的洞误读为 0 而非缺失属性（与 `new Array(len)` 语义对齐）。
fn array_allocate(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(encoded) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(length) = to_number(state, encoded) else {
        return fail_dispatch(ctx);
    };
    if !length.is_finite() || length < 0.0 || length > f64::from(u32::MAX) || length.fract() != 0.0
    {
        return fail_dispatch(ctx);
    }
    let length = length as u32;
    let Ok(array) = state.allocate_object(length, true) else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(array);
    for index in 0..length {
        if state
            .heap
            .set_element(handle, index, value::encode_array_hole() as u64)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    if length != 0
        && state
            .heap
            .raise_array_kind(handle, wjsm_ir::constants::ARRAY_KIND_HOLEY)
            .is_err()
    {
        return fail_dispatch(ctx);
    }
    if state.heap.set_array_length(handle, length).is_err() {
        return fail_dispatch(ctx);
    }
    array
}

/// `array.has_element(array, index)`：数组索引处存在非 hole 元素 → bool。
fn array_has_element(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let [array, index] = args else {
        return fail_dispatch(ctx);
    };
    if !value::is_array(*array) {
        return fail_dispatch(ctx);
    }
    let handle = value::decode_handle(*array);
    let Some(index) = array_index(state, *index) else {
        return value::encode_bool(false);
    };
    match state.heap.get_element(handle, index) {
        Ok(Some(stored)) => value::encode_bool(!value::is_array_hole(stored as i64)),
        Ok(None) => value::encode_bool(false),
        Err(_) => fail_dispatch(ctx),
    }
}

fn integer(state: &NativeAgentState, encoded: Option<i64>, default: i64) -> Option<i64> {
    let Some(encoded) = encoded else {
        return Some(default);
    };
    let number = to_number(state, encoded)?;
    if number.is_nan() || number == 0.0 {
        Some(0)
    } else if number >= i64::MAX as f64 {
        Some(i64::MAX)
    } else if number <= i64::MIN as f64 {
        Some(i64::MIN)
    } else {
        Some(number.trunc() as i64)
    }
}

fn relative(index: i64, length: u32) -> u32 {
    let length = i64::from(length);
    if index < 0 {
        (length + index).max(0) as u32
    } else {
        index.min(length) as u32
    }
}

fn array_push(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    for stored in &args[1..] {
        if state.heap.push_element(handle, *stored as u64).is_err() {
            return fail_dispatch(ctx);
        }
    }
    array_length(ctx, state, args)
}

fn array_push_hole(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    state
        .heap
        .push_element(handle, value::encode_array_hole() as u64)
        .map(|length| value::encode_f64(f64::from(length)))
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn array_push_spread(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [target, source] = args else {
        return fail_dispatch(ctx);
    };
    if !value::is_array(*target) {
        return fail_dispatch(ctx);
    }
    let target = value::decode_handle(*target);
    let iterator = super::runtime::iterator_from(ctx, state, &[*source]);
    if value::is_exception(iterator) {
        return iterator;
    }
    loop {
        let done = super::runtime::iterator_done(ctx, state, &[iterator]);
        if value::is_exception(done) {
            return done;
        }
        if super::runtime::is_truthy(state, done) {
            break;
        }
        let stored = super::runtime::iterator_value(ctx, state, &[iterator], true);
        if value::is_exception(stored) {
            return stored;
        }
        if state.heap.push_element(target, stored as u64).is_err() {
            return fail_dispatch(ctx);
        }
    }
    state
        .array_iterators
        .remove(&value::decode_handle(iterator));
    value::encode_f64(f64::from(length(state, target).unwrap_or(0)))
}

fn array_pop(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    if length == 0 {
        return value::encode_undefined();
    }
    let result = get(state, handle, length - 1);
    state
        .heap
        .set_array_length(handle, length - 1)
        .map(|()| result)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn array_splice(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let start = relative(integer(state, args.get(1).copied(), 0).unwrap_or(0), length);
    let available = length - start;
    let delete_count = if args.len() < 3 {
        available
    } else {
        integer(state, args.get(2).copied(), 0)
            .unwrap_or(0)
            .clamp(0, i64::from(available)) as u32
    };
    let Ok(item_count) = u32::try_from(args.len().saturating_sub(3)) else {
        return fail_dispatch(ctx);
    };
    let Some(new_length) = length
        .checked_sub(delete_count)
        .and_then(|length| length.checked_add(item_count))
    else {
        return super::runtime::range_error(ctx, state, "Invalid array length");
    };
    let removed = (start..start + delete_count)
        .map(|index| get_raw(state, handle, index).unwrap_or_else(value::encode_array_hole))
        .collect::<Vec<_>>();
    let removed = match state.allocate_array_values(&removed) {
        Ok(removed) => removed,
        Err(_) => return fail_dispatch(ctx),
    };

    if item_count < delete_count {
        for source in start + delete_count..length {
            let stored = get_raw(state, handle, source).unwrap_or_else(value::encode_array_hole);
            if !set(state, handle, source - delete_count + item_count, stored) {
                return fail_dispatch(ctx);
            }
        }
    } else if item_count > delete_count {
        for source in (start + delete_count..length).rev() {
            let stored = get_raw(state, handle, source).unwrap_or_else(value::encode_array_hole);
            if !set(state, handle, source - delete_count + item_count, stored) {
                return fail_dispatch(ctx);
            }
        }
    }
    for (offset, stored) in args.get(3..).unwrap_or_default().iter().enumerate() {
        if !set(state, handle, start + offset as u32, *stored) {
            return fail_dispatch(ctx);
        }
    }
    if state.heap.set_array_length(handle, new_length).is_err() {
        return fail_dispatch(ctx);
    }
    removed
}

fn array_to_spliced(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let start = relative(integer(state, args.get(1).copied(), 0).unwrap_or(0), length);
    let available = length - start;
    let delete_count = if args.len() < 3 {
        available
    } else {
        integer(state, args.get(2).copied(), 0)
            .unwrap_or(0)
            .clamp(0, i64::from(available)) as u32
    };
    let item_count = args.len().saturating_sub(3);
    let Some(capacity) = usize::try_from(length - delete_count)
        .ok()
        .and_then(|length| length.checked_add(item_count))
    else {
        return super::runtime::range_error(ctx, state, "Invalid array length");
    };
    let mut values = Vec::with_capacity(capacity);
    for index in 0..start {
        values.push(get(state, handle, index));
    }
    values.extend_from_slice(args.get(3..).unwrap_or_default());
    for index in start + delete_count..length {
        values.push(get(state, handle, index));
    }
    state
        .allocate_array_values(&values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn array_flat(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let depth = integer(state, args.get(1).copied(), 1).unwrap_or(0).max(0) as u32;
    let mut values = Vec::new();
    if !flatten_into(state, handle, depth, &mut values) {
        return fail_dispatch(ctx);
    }
    state
        .allocate_array_values(&values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn flatten_into(state: &NativeAgentState, handle: u32, depth: u32, values: &mut Vec<i64>) -> bool {
    let Some(length) = length(state, handle) else {
        return false;
    };
    for index in 0..length {
        let Some(stored) =
            get_raw(state, handle, index).filter(|stored| !value::is_array_hole(*stored))
        else {
            continue;
        };
        if depth != 0 && value::is_array(stored) {
            if !flatten_into(state, value::decode_handle(stored), depth - 1, values) {
                return false;
            }
        } else {
            values.push(stored);
        }
    }
    true
}

#[derive(Clone, Copy)]
enum SearchKind {
    Includes,
    IndexOf,
    LastIndexOf,
}

fn array_search(
    ctx: &mut NativeVmContext,
    state: &NativeAgentState,
    args: &[i64],
    kind: SearchKind,
) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let search = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let default_from = if matches!(kind, SearchKind::LastIndexOf) {
        i64::from(length).saturating_sub(1)
    } else {
        0
    };
    let Some(from) = integer(state, args.get(2).copied(), default_from) else {
        return fail_dispatch(ctx);
    };
    let matches = |element: i64| {
        strict_equal(state, element, search)
            || (matches!(kind, SearchKind::Includes)
                && value::is_f64(element)
                && value::is_f64(search)
                && value::decode_f64(element).is_nan()
                && value::decode_f64(search).is_nan())
    };
    let found = if matches!(kind, SearchKind::LastIndexOf) {
        let mut index = if from >= 0 {
            from.min(i64::from(length).saturating_sub(1))
        } else {
            i64::from(length) + from
        };
        let mut found = None;
        while index >= 0 {
            let raw = get_raw(state, handle, index as u32);
            if raw.is_some_and(|element| !value::is_array_hole(element) && matches(element)) {
                found = Some(index as u32);
                break;
            }
            index -= 1;
        }
        found
    } else {
        let mut found = None;
        for index in relative(from, length)..length {
            let raw = get_raw(state, handle, index);
            let element = raw
                .filter(|element| !value::is_array_hole(*element))
                .unwrap_or_else(value::encode_undefined);
            if (matches!(kind, SearchKind::Includes)
                || raw.is_some_and(|element| !value::is_array_hole(element)))
                && matches(element)
            {
                found = Some(index);
                break;
            }
        }
        found
    };
    if matches!(kind, SearchKind::Includes) {
        value::encode_bool(found.is_some())
    } else {
        value::encode_f64(found.map_or(-1.0, f64::from))
    }
}

fn array_join(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let separator = match args.get(1).copied() {
        None => ",".into(),
        Some(separator) if value::is_undefined(separator) => ",".into(),
        Some(separator) => match to_string_coerced(ctx, state, separator) {
            Ok(separator) => separator,
            Err(exception) => return exception,
        },
    };
    let length = match array_like_length(ctx, state, receiver) {
        Ok(length) => length,
        Err(exception) => return exception,
    };
    let mut result = String::new();
    for index in 0..length {
        if index != 0 {
            result.push_str(&separator);
        }
        let element = if value::is_array(receiver) {
            get(state, value::decode_handle(receiver), index)
        } else {
            let Some(key) = state.intern_text(index.to_string(), value::TAG_STRING) else {
                return fail_dispatch(ctx);
            };
            match get_property(ctx, state, receiver, key) {
                Ok(element) => element,
                Err(()) => return fail_dispatch(ctx),
            }
        };
        if value::is_exception(element) {
            return element;
        }
        if value::is_null(element) || value::is_undefined(element) {
            continue;
        }
        match to_string_coerced(ctx, state, element) {
            Ok(element) => result.push_str(&element),
            Err(exception) => return exception,
        }
    }
    state
        .intern_runtime_string(RuntimeString::from(result), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn array_concat(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(first) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let spreadable_key =
        value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::IS_CONCAT_SPREADABLE);
    let mut values = Vec::new();
    for item in std::iter::once(&first).chain(args[1..].iter()) {
        let spreadable = match get_property(ctx, state, *item, spreadable_key) {
            Ok(property) if value::is_exception(property) => return property,
            Ok(property) if !value::is_undefined(property) => is_truthy(state, property),
            Ok(_) => value::is_array(*item),
            Err(()) => return fail_dispatch(ctx),
        };
        if !spreadable {
            values.push(*item);
            continue;
        }
        let source_length = match array_like_length(ctx, state, *item) {
            Ok(length) => length,
            Err(exception) => return exception,
        };
        for index in 0..source_length {
            if value::is_array(*item) {
                values.push(
                    get_raw(state, value::decode_handle(*item), index)
                        .unwrap_or_else(value::encode_array_hole),
                );
                continue;
            }
            let key = value::encode_f64(f64::from(index));
            if !has_property(state, *item, key) {
                values.push(value::encode_array_hole());
                continue;
            }
            match get_property(ctx, state, *item, key) {
                Ok(stored) => values.push(stored),
                Err(()) => return fail_dispatch(ctx),
            }
        }
    }
    state
        .allocate_array_values(&values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn array_slice(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let length = match array_like_length(ctx, state, receiver) {
        Ok(length) => length,
        Err(exception) => return exception,
    };
    let start = relative(integer(state, args.get(1).copied(), 0).unwrap_or(0), length);
    let end = relative(
        integer(state, args.get(2).copied(), i64::from(length)).unwrap_or(i64::from(length)),
        length,
    );
    let values = match slice_values(ctx, state, receiver, start, end.max(start)) {
        Ok(values) => values,
        Err(exception) => return exception,
    };
    state
        .allocate_array_values(&values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn array_like_length(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
) -> Result<u32, i64> {
    if value::is_array(receiver) {
        return length(state, value::decode_handle(receiver)).ok_or_else(|| fail_dispatch(ctx));
    }
    let key = state
        .intern_text("length".into(), value::TAG_STRING)
        .ok_or_else(|| fail_dispatch(ctx))?;
    let stored = get_property(ctx, state, receiver, key).map_err(|_| fail_dispatch(ctx))?;
    if value::is_exception(stored) {
        return Err(stored);
    }
    let number = to_number(state, stored).unwrap_or(0.0);
    Ok(if !number.is_finite() || number <= 0.0 {
        0
    } else {
        number.trunc().min(f64::from(u32::MAX)) as u32
    })
}

fn slice_values(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    start: u32,
    end: u32,
) -> Result<Vec<i64>, i64> {
    if value::is_array(receiver) {
        let handle = value::decode_handle(receiver);
        return Ok((start..end)
            .map(|index| get_raw(state, handle, index).unwrap_or_else(value::encode_array_hole))
            .collect());
    }
    let mut values = Vec::with_capacity((end - start) as usize);
    for index in start..end {
        let key = state
            .intern_text(index.to_string(), value::TAG_STRING)
            .ok_or_else(|| fail_dispatch(ctx))?;
        if value::is_string(receiver) || has_property(state, receiver, key) {
            let stored = get_property(ctx, state, receiver, key).map_err(|_| fail_dispatch(ctx))?;
            if value::is_exception(stored) {
                return Err(stored);
            }
            values.push(stored);
        } else {
            values.push(value::encode_array_hole());
        }
    }
    Ok(values)
}

fn array_fill(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let stored = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let start = relative(integer(state, args.get(2).copied(), 0).unwrap_or(0), length);
    let end = relative(
        integer(state, args.get(3).copied(), i64::from(length)).unwrap_or(i64::from(length)),
        length,
    );
    for index in start..end.max(start) {
        if !set(state, handle, index, stored) {
            return fail_dispatch(ctx);
        }
    }
    args[0]
}

fn array_reverse(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    for lower in 0..length / 2 {
        let upper = length - lower - 1;
        let left = get_raw(state, handle, lower).unwrap_or_else(value::encode_array_hole);
        let right = get_raw(state, handle, upper).unwrap_or_else(value::encode_array_hole);
        if !set(state, handle, lower, right) || !set(state, handle, upper, left) {
            return fail_dispatch(ctx);
        }
    }
    args[0]
}

fn array_set_length(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let [array, requested] = args else {
        return fail_dispatch(ctx);
    };
    if !value::is_array(*array) {
        return fail_dispatch(ctx);
    }
    let Some(requested) = to_number(state, *requested)
        .filter(|number| {
            number.is_finite()
                && *number >= 0.0
                && number.fract() == 0.0
                && *number <= u32::MAX as f64
        })
        .map(|number| number as u32)
    else {
        return fail_dispatch(ctx);
    };
    state
        .heap
        .set_array_length(value::decode_handle(*array), requested)
        .map(|()| *array)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn array_length(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    handle(args)
        .and_then(|handle| length(state, handle))
        .map(|length| value::encode_f64(f64::from(length)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn array_shift(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    if length == 0 {
        return value::encode_undefined();
    }
    let first = get(state, handle, 0);
    for index in 1..length {
        let stored = get_raw(state, handle, index).unwrap_or_else(value::encode_array_hole);
        if !set(state, handle, index - 1, stored) {
            return fail_dispatch(ctx);
        }
    }
    state
        .heap
        .set_array_length(handle, length - 1)
        .map(|()| first)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn array_unshift(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let count = args.len().saturating_sub(1) as u32;
    let Some(new_length) = length.checked_add(count) else {
        return fail_dispatch(ctx);
    };
    if state.heap.set_array_length(handle, new_length).is_err() {
        return fail_dispatch(ctx);
    }
    for index in (0..length).rev() {
        let stored = get_raw(state, handle, index).unwrap_or_else(value::encode_array_hole);
        if !set(state, handle, index + count, stored) {
            return fail_dispatch(ctx);
        }
    }
    for (index, stored) in args[1..].iter().enumerate() {
        if !set(state, handle, index as u32, *stored) {
            return fail_dispatch(ctx);
        }
    }
    value::encode_f64(f64::from(new_length))
}

fn array_at(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let Some(mut index) = integer(state, args.get(1).copied(), 0) else {
        return fail_dispatch(ctx);
    };
    if index < 0 {
        index += i64::from(length);
    }
    usize::try_from(index)
        .ok()
        .filter(|index| *index < length as usize)
        .map(|index| get(state, handle, index as u32))
        .unwrap_or_else(value::encode_undefined)
}

fn array_copy_within(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let target = relative(integer(state, args.get(1).copied(), 0).unwrap_or(0), length);
    let start = relative(integer(state, args.get(2).copied(), 0).unwrap_or(0), length);
    let end = relative(
        integer(state, args.get(3).copied(), i64::from(length)).unwrap_or(i64::from(length)),
        length,
    );
    let count = end.saturating_sub(start).min(length.saturating_sub(target));
    let values: Vec<_> = (0..count)
        .map(|offset| {
            get_raw(state, handle, start + offset).unwrap_or_else(value::encode_array_hole)
        })
        .collect();
    for (offset, stored) in values.into_iter().enumerate() {
        if !set(state, handle, target + offset as u32, stored) {
            return fail_dispatch(ctx);
        }
    }
    args[0]
}

fn array_to_reversed(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let values: Vec<_> = (0..length)
        .rev()
        .map(|index| get(state, handle, index))
        .collect();
    state
        .allocate_array_values(&values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn array_with(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(length) = length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let Some(mut index) = integer(state, args.get(1).copied(), 0) else {
        return fail_dispatch(ctx);
    };
    if index < 0 {
        index += i64::from(length);
    }
    if index < 0 || index >= i64::from(length) {
        return fail_dispatch(ctx);
    }
    let replacement = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    let values: Vec<_> = (0..length)
        .map(|position| {
            if i64::from(position) == index {
                replacement
            } else {
                get(state, handle, position)
            }
        })
        .collect();
    state
        .allocate_array_values(&values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}
