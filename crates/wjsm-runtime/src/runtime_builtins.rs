use super::*;
use crate::runtime_string::RuntimeString;

#[derive(Clone, Copy)]
pub(crate) enum PromiseSettlement {
    Fulfill(i64),
    Reject(i64),
}

pub(crate) fn raw_promise_handle(promise: i64) -> usize {
    if value::is_object(promise) {
        value::decode_object_handle(promise) as usize
    } else {
        promise as usize
    }
}
fn invoke_string_primitive_method(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    method: u8,
    args: &[i64],
) -> i64 {
    let receiver = get_string_value(caller, this_val);
    let search = args
        .first()
        .copied()
        .map(|value| crate::runtime_encoding::js_string_value(caller, value))
        .unwrap_or_else(|| RuntimeString::from_utf8_str("undefined"));
    let position = args
        .get(1)
        .copied()
        .map(|value| value::decode_f64(to_number(caller, value)))
        .filter(|number| number.is_finite() && *number > 0.0)
        .map(|number| number.trunc() as usize)
        .unwrap_or(0)
        .min(receiver.utf16_len());
    match method {
        0 => value::encode_bool(receiver.find_units(&search, position).is_some()),
        1 => value::encode_bool(receiver.starts_with_units(&search, position)),
        2 => receiver
            .find_units(&search, position)
            .map(|index| value::encode_f64(index as f64))
            .unwrap_or_else(|| value::encode_f64(-1.0)),
        _ => value::encode_undefined(),
    }
}

pub(crate) fn insert_promise_entry(
    table: &mut Vec<PromiseEntry>,
    handle: usize,
    entry: PromiseEntry,
) {
    if table.len() <= handle {
        table.resize_with(handle + 1, PromiseEntry::empty);
    }
    table[handle] = entry;
}

pub(crate) async fn call_iterable_method_async(
    caller: &mut Caller<'_, RuntimeState>,
    method: i64,
    receiver: i64,
) -> i64 {
    if value::is_native_callable(method) {
        return call_native_callable_with_args_from_caller_async(caller, method, receiver, vec![])
            .await
            .unwrap_or_else(value::encode_undefined);
    }
    // 若调用返回 Err 且包装了 TAG_EXCEPTION，需要保留异常而非转换成 undefined
    match call_wasm_callback_async(caller, method, receiver, &[]).await {
        Ok(val) => val,
        Err(_) => {
            // 通常 call_wasm_callback_async 的 Err 表示陷阱/panic，此时回传 undefined。
            // 但若 method 本身不可调用，GetMethod 已在上游返回 Err(TAG_EXCEPTION)，
            // 该路径不会走到这里（GetMethod 的 Err 在 lib.rs 的 match 中被处理）。
            // 这里保持原有语义：调用失败 → undefined。
            value::encode_undefined()
        }
    }
}

pub(crate) async fn call_iterator_method_async(
    caller: &mut Caller<'_, RuntimeState>,
    method: i64,
    iterator: i64,
    argument: i64,
) -> i64 {
    if value::is_native_callable(method) {
        return Box::pin(call_native_callable_with_args_from_caller_async(
            caller,
            method,
            iterator,
            if value::is_undefined(argument) {
                vec![]
            } else {
                vec![argument]
            },
        ))
        .await
        .unwrap_or_else(value::encode_undefined);
    }
    let args = if value::is_undefined(argument) {
        &[][..]
    } else {
        std::slice::from_ref(&argument)
    };
    match call_wasm_callback_async(caller, method, iterator, args).await {
        Ok(val) => val,
        Err(_) => value::encode_undefined(),
    }
}

pub(crate) async fn advance_object_iterator_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    iterator: i64,
    next: i64,
) -> (i64, i64, bool, bool) {
    let result =
        call_iterator_method_async(caller, next, iterator, value::encode_undefined()).await;

    // A3: 若 next() 同步抛出（TAG_EXCEPTION），原样回传让上游处理（转 rejected promise）。
    if value::is_exception(result) {
        return (result, value::encode_undefined(), false, true);
    }

    let mut result = result;
    if is_promise_value(caller.data(), result) {
        let promise_handle = raw_promise_handle(result);
        let (fulfilled, rejected) = {
            let table_p = caller
                .data()
                .promise_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match promise_entry(&table_p, promise_handle).map(|e| &e.state) {
                Some(PromiseState::Fulfilled(v)) => (Some(*v), None),
                Some(PromiseState::Rejected(r)) => (None, Some(*r)),
                _ => (None, None),
            }
        };
        if rejected.is_some() {
            return (result, value::encode_undefined(), false, false);
        }
        if let Some(settled_val) = fulfilled {
            result = settled_val;
        } else {
            return (result, value::encode_undefined(), false, false);
        }
    }
    if (value::is_object(result) || value::is_function(result) || value::is_array(result))
        && let Some(ptr) = resolve_handle(caller, result)
    {
        let done = read_object_property_by_name(caller, ptr, "done")
            .map(nanbox_to_bool)
            .unwrap_or(false);
        let current_value = read_object_property_by_name(caller, ptr, "value")
            .unwrap_or_else(value::encode_undefined);
        return (result, current_value, done, true);
    }

    // 非对象非异常 → 构造可捕获 TypeError
    let type_error = make_type_error_exception(caller, "iterator next must return an object");
    (type_error, value::encode_undefined(), false, true)
}

pub(crate) fn create_async_generator_identity(state: &RuntimeState, generator: i64) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(NativeCallable::AsyncGeneratorIdentity { generator });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn create_iterator_proto_identity(state: &RuntimeState) -> i64 {
    create_native_callable(state, NativeCallable::IteratorProtoSymbolIterator)
}

pub(crate) fn create_map_set_method(state: &RuntimeState, kind: MapSetMethodKind) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(NativeCallable::MapSetMethod { kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn create_date_method(state: &RuntimeState, kind: DateMethodKind) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(NativeCallable::DateMethod { kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn create_weakmap_method(state: &RuntimeState, kind: WeakMapMethodKind) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(NativeCallable::WeakMapMethod { kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn create_weakset_method(state: &RuntimeState, kind: WeakSetMethodKind) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(NativeCallable::WeakSetMethod { kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn read_weakmap_handle(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> Option<usize> {
    if !value::is_object(this_val) {
        return None;
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let op = obj_ptr?;
    let handle_val = read_object_property_by_name(caller, op, "__weakmap_handle__")?;
    Some(value::decode_f64(handle_val) as usize)
}

pub(crate) fn read_weakset_handle(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> Option<usize> {
    if !value::is_object(this_val) {
        return None;
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let op = obj_ptr?;
    let handle_val = read_object_property_by_name(caller, op, "__weakset_handle__")?;
    Some(value::decode_f64(handle_val) as usize)
}

/// `BigIntPrimitiveMethod`：this 为 bigint handle，按 method 调用 BigInt.prototype 语义。
fn invoke_bigint_primitive_method(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    method: u8,
    args: &[i64],
) -> i64 {
    if !value::is_bigint(this_val) {
        return value::encode_undefined();
    }
    let handle = value::decode_bigint_handle(this_val) as usize;
    let radix_or_digits = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    match method {
        0 => {
            let radix = if value::is_undefined(radix_or_digits) || value::is_null(radix_or_digits) {
                10
            } else if value::is_f64(radix_or_digits) {
                let r = value::decode_f64(radix_or_digits) as i32;
                if !(2..=36).contains(&r) {
                    set_runtime_error(
                        caller.data(),
                        "RangeError: toString() radix argument must be between 2 and 36"
                            .to_string(),
                    );
                    return value::encode_undefined();
                }
                r
            } else {
                10
            };
            let table = caller
                .data()
                .bigint_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(bi) = table.get(handle) else {
                return store_runtime_string(caller, "0".to_string());
            };
            let s = if radix == 10 {
                bi.to_string()
            } else {
                bi.to_str_radix(radix as u32)
            };
            store_runtime_string(caller, s)
        }
        1 => this_val,
        _ => value::encode_undefined(),
    }
}

fn invoke_symbol_primitive_method(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    method: u8,
) -> i64 {
    match method {
        0 => symbol_proto_to_string_impl(caller, this_val),
        1 => symbol_proto_value_of_impl(caller, this_val),
        _ => value::encode_undefined(),
    }
}

/// `NumberPrimitiveMethod`：this 为 raw f64，按 method 调用已有 number_proto 语义。
fn invoke_number_primitive_method(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    method: u8,
    args: &[i64],
) -> i64 {
    if !value::is_f64(this_val) {
        return value::encode_undefined();
    }
    let radix_or_digits = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    match method {
        0 => {
            // toString — 复用与 number_proto_to_string 相同逻辑（radix 默认 10）
            let x = value::decode_f64(this_val);
            let radix = if value::is_undefined(radix_or_digits) || value::is_null(radix_or_digits) {
                10
            } else if value::is_f64(radix_or_digits) {
                let r = value::decode_f64(radix_or_digits) as i32;
                if !(2..=36).contains(&r) {
                    return make_range_error_exception(
                        caller,
                        "toString() radix argument must be between 2 and 36",
                    );
                }
                r
            } else {
                10
            };
            if x.is_nan() {
                return store_runtime_string(caller, "NaN".to_string());
            }
            if x.is_infinite() {
                return store_runtime_string(
                    caller,
                    if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string(),
                );
            }
            if radix == 10 {
                return store_runtime_string(caller, format_number_js(x));
            }
            store_runtime_string(caller, format_f64_radix_to_string(x, radix))
        }
        1 => this_val,
        2 => {
            let x = value::decode_f64(this_val);
            let digits = if value::is_undefined(radix_or_digits) || value::is_null(radix_or_digits)
            {
                0
            } else if value::is_f64(radix_or_digits) {
                value::decode_f64(radix_or_digits) as i32
            } else {
                0
            };
            if !(0..=100).contains(&digits) {
                return make_range_error_exception(
                    caller,
                    "toFixed() digits argument must be between 0 and 100",
                );
            }
            store_runtime_string(caller, format_number_to_fixed_js(x, digits))
        }
        3 => {
            let x = value::decode_f64(this_val);
            let digits = if value::is_undefined(radix_or_digits) || value::is_null(radix_or_digits)
            {
                None
            } else if value::is_f64(radix_or_digits) {
                Some(value::decode_f64(radix_or_digits) as i32)
            } else {
                None
            };
            // ECMA-262 §21.1.3.7 step 5: f < 0 或 f > 100 → RangeError
            if let Some(f) = digits
                && !(0..=100).contains(&f)
            {
                return make_range_error_exception(
                    caller,
                    "toExponential() argument must be between 0 and 100",
                );
            }
            store_runtime_string(caller, format_number_to_exponential_js(x, digits))
        }
        4 => {
            let x = value::decode_f64(this_val);
            let precision = if value::is_undefined(radix_or_digits) {
                None
            } else if value::is_f64(radix_or_digits) {
                Some(value::decode_f64(radix_or_digits) as i32)
            } else {
                Some(-1)
            };
            if let Some(precision) = precision
                && !(1..=100).contains(&precision)
            {
                return make_range_error_exception(
                    caller,
                    "toPrecision() argument must be between 1 and 100",
                );
            }
            store_runtime_string(caller, format_number_to_precision_js(x, precision))
        }
        _ => value::encode_undefined(),
    }
}

fn array_like_length(caller: &mut Caller<'_, RuntimeState>, target: i64) -> u32 {
    if value::is_array(target) {
        return caller
            .data()
            .heap_access_v2()
            .array_length(value::decode_handle(target))
            .unwrap_or(0);
    }
    if value::is_array(target)
        && let Some(ptr) = resolve_handle(caller, target)
    {
        return read_array_length(caller, ptr).unwrap_or(0);
    }
    let Some(ptr) = resolve_handle(caller, target) else {
        return 0;
    };
    read_object_property_by_name(caller, ptr, "length")
        .map(value::decode_f64)
        .unwrap_or(0.0)
        .max(0.0) as u32
}

fn create_array_like_iterator(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    kind: ArrayIterKind,
) -> i64 {
    let length = array_like_length(caller, target);
    let index = Arc::new(Mutex::new(0));
    let next = create_native_callable(
        caller.data(),
        NativeCallable::ArrayLikeIteratorNext {
            target,
            index,
            length,
            kind,
        },
    );
    let obj = alloc_host_object_v2(caller, 2);
    let self_fn = create_native_callable(caller.data(), NativeCallable::RegExpStringIteratorSelf);
    let _ = define_host_data_property_from_caller(caller, obj, "next", next);
    let _ = define_host_data_property_by_name_id(
        caller,
        obj,
        encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR),
        self_fn,
    );
    obj
}

pub(crate) fn create_raw_iterator_object(
    caller: &mut Caller<'_, RuntimeState>,
    iterator: i64,
) -> i64 {
    let next = create_native_callable(caller.data(), NativeCallable::RawIteratorNext { iterator });
    let self_fn = create_native_callable(caller.data(), NativeCallable::RegExpStringIteratorSelf);
    let obj = alloc_host_object_v2(caller, 2);
    let _ = define_host_data_property_from_caller(caller, obj, "next", next);
    let _ = define_host_data_property_by_name_id(
        caller,
        obj,
        encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR),
        self_fn,
    );
    obj
}

fn raw_iterator_next_result(caller: &mut Caller<'_, RuntimeState>, iterator: i64) -> i64 {
    if !value::is_iterator(iterator) {
        return alloc_iterator_result_from_caller(caller, value::encode_undefined(), true);
    }
    let handle_idx = value::decode_handle(iterator) as usize;
    if raw_iterator_done(caller, handle_idx) {
        return alloc_iterator_result_from_caller(caller, value::encode_undefined(), true);
    }
    let current = iterator_value_impl(caller, iterator);
    advance_raw_iterator(caller, handle_idx);
    alloc_iterator_result_from_caller(caller, current, false)
}

fn raw_iterator_done(caller: &mut Caller<'_, RuntimeState>, handle_idx: usize) -> bool {
    let mut iters = caller
        .data()
        .iterators
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(iter) = iters.get_mut(handle_idx) else {
        return true;
    };
    match iter {
        IteratorState::StringIter { string, unit_pos } => *unit_pos >= string.utf16_len(),
        IteratorState::ArrayIter { index, length, .. } => *index >= *length,
        IteratorState::MapKeyIter {
            index, map_handle, ..
        }
        | IteratorState::MapValueIter {
            index, map_handle, ..
        }
        | IteratorState::MapEntryIter {
            index, map_handle, ..
        } => {
            let table = caller
                .data()
                .map_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *map_handle >= table.len() as u32
                || *index as usize >= table[*map_handle as usize].keys.len()
        }
        IteratorState::SetValueIter {
            index, set_handle, ..
        }
        | IteratorState::SetEntryIter {
            index, set_handle, ..
        } => {
            let table = caller
                .data()
                .set_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *set_handle >= table.len() as u32
                || *index as usize >= table[*set_handle as usize].values.len()
        }
        IteratorState::HeadersKeyIter {
            index,
            headers_handle,
        }
        | IteratorState::HeadersValueIter {
            index,
            headers_handle,
        }
        | IteratorState::HeadersEntryIter {
            index,
            headers_handle,
        } => {
            let table = caller
                .data()
                .headers_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *headers_handle >= table.len() as u32
                || *index as usize >= table[*headers_handle as usize].pairs.len()
        }
        IteratorState::IndexValueIter { index, values } => *index as usize >= values.len(),
        IteratorState::TypedArrayValueIter { index, length, .. }
        | IteratorState::TypedArrayEntryIter { index, length, .. } => *index >= *length,
        IteratorState::RegExpStringIter { .. } => {
            drop(iters);
            regexp_string_iter_ensure_current(caller, handle_idx)
        }
        IteratorState::ObjectIter { done, .. } => *done,
        IteratorState::Error => true,
    }
}

fn advance_raw_iterator(caller: &mut Caller<'_, RuntimeState>, handle_idx: usize) {
    let mut iters = caller
        .data()
        .iterators
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(iter) = iters.get_mut(handle_idx) else {
        return;
    };
    match iter {
        IteratorState::StringIter { string, unit_pos } => {
            string_iter_advance_unit_pos(string, unit_pos)
        }
        IteratorState::ArrayIter { index, .. }
        | IteratorState::MapKeyIter { index, .. }
        | IteratorState::MapValueIter { index, .. }
        | IteratorState::MapEntryIter { index, .. }
        | IteratorState::SetValueIter { index, .. }
        | IteratorState::SetEntryIter { index, .. }
        | IteratorState::HeadersKeyIter { index, .. }
        | IteratorState::HeadersValueIter { index, .. }
        | IteratorState::HeadersEntryIter { index, .. }
        | IteratorState::IndexValueIter { index, .. }
        | IteratorState::TypedArrayValueIter { index, .. }
        | IteratorState::TypedArrayEntryIter { index, .. } => {
            *index += 1;
        }
        IteratorState::RegExpStringIter { .. } => {
            drop(iters);
            regexp_string_iter_next(caller, handle_idx);
        }
        IteratorState::ObjectIter { .. } | IteratorState::Error => {}
    }
}

fn advance_array_like_iterator(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    index: Arc<Mutex<u32>>,
    length: u32,
    kind: ArrayIterKind,
) -> i64 {
    let idx = {
        let mut index = index.lock().unwrap_or_else(|error| error.into_inner());
        if *index >= length {
            return alloc_iterator_result_from_caller(caller, value::encode_undefined(), true);
        }
        let idx = *index;
        *index += 1;
        idx
    };
    let element = if value::is_array(target) {
        {
            caller
                .data()
                .heap_access_v2()
                .get_element(value::decode_handle(target), idx)
                .ok()
                .flatten()
                .map(|element| element as i64)
                .unwrap_or_else(value::encode_undefined)
        }
    } else {
        let key = idx.to_string();
        resolve_handle(caller, target)
            .and_then(|pointer| read_object_property_by_name(caller, pointer, &key))
            .unwrap_or_else(value::encode_undefined)
    };
    let produced = match kind {
        ArrayIterKind::Keys => value::encode_f64(idx as f64),
        ArrayIterKind::Values => element,
        ArrayIterKind::Entries => {
            let entry = alloc_array(caller, 2);
            if let Some(entry_ptr) = resolve_array_ptr(caller, entry) {
                write_array_elem(caller, entry_ptr, 0, value::encode_f64(idx as f64));
                write_array_elem(caller, entry_ptr, 1, element);
                write_array_length(caller, entry_ptr, 2);
            }
            entry
        }
    };
    alloc_iterator_result_from_caller(caller, produced, false)
}

pub(crate) fn call_native_callable_with_args_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    callable: i64,
    this_val: i64,
    args: Vec<i64>,
) -> Option<i64> {
    if !value::is_native_callable(callable) {
        return None;
    }

    let idx = value::decode_native_callable_idx(callable) as usize;
    let record = {
        let table = caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table.get(idx).cloned()
    }?;
    let argument = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);

    match record {
        NativeCallable::ArgumentsStrictCalleeGetter => Some(
            crate::runtime_arguments::arguments_strict_callee_getter(caller, this_val),
        ),
        NativeCallable::ArrayProtoValues => Some(create_array_like_iterator(
            caller,
            this_val,
            ArrayIterKind::Values,
        )),
        NativeCallable::ArrayProtoKeys => Some(create_array_like_iterator(
            caller,
            this_val,
            ArrayIterKind::Keys,
        )),
        NativeCallable::ArrayProtoEntries => Some(create_array_like_iterator(
            caller,
            this_val,
            ArrayIterKind::Entries,
        )),
        NativeCallable::ArrayLikeIteratorNext {
            target,
            index,
            length,
            kind,
        } => Some(advance_array_like_iterator(
            caller, target, index, length, kind,
        )),
        NativeCallable::RawIteratorNext { iterator } => {
            Some(raw_iterator_next_result(caller, iterator))
        }
        NativeCallable::BigIntPrimitiveMethod { method } => Some(invoke_bigint_primitive_method(
            caller, this_val, method, &args,
        )),
        NativeCallable::NumberPrimitiveMethod { method } => Some(invoke_number_primitive_method(
            caller, this_val, method, &args,
        )),
        NativeCallable::StringPrimitiveMethod { method } => Some(invoke_string_primitive_method(
            caller, this_val, method, &args,
        )),
        NativeCallable::SymbolPrimitiveMethod { method } => {
            Some(invoke_symbol_primitive_method(caller, this_val, method))
        }
        NativeCallable::RegExpPrimitiveMethod { method } => Some(invoke_regexp_primitive_method(
            caller, this_val, method, &args,
        )),
        NativeCallable::RegExpStringIteratorNext { iter_handle } => Some(
            crate::runtime_regexp::regexp_string_iterator_step(caller, iter_handle),
        ),
        NativeCallable::RegExpStringIteratorSelf => Some(this_val),
        NativeCallable::SymbolProtoDescriptionGetter => {
            Some(symbol_proto_description_getter_impl(caller, this_val))
        }
        NativeCallable::SymbolProtoToPrimitive => {
            Some(symbol_proto_value_of_impl(caller, this_val))
        }
        NativeCallable::FunctionConstructor => {
            // Function 构造器仅走 async 分派（需解析 body）。
            set_runtime_error(
                caller.data(),
                "Function constructor unsupported on sync NativeCallable path".to_string(),
            );
            Some(value::encode_undefined())
        }
        NativeCallable::EvalIndirect | NativeCallable::EvalFunction(_) => {
            // sync 路径已退役（参见 docs/async-scheduler.md / async_reentry_audit）；
            // 唯一进入点是 sync eval 解释器内嵌套 eval，本身已改为错误返回。
            set_runtime_error(
                caller.data(),
                "indirect eval / eval function unsupported on sync NativeCallable path".to_string(),
            );
            Some(value::encode_undefined())
        }

        NativeCallable::CjsRequire { referrer } => Some(call_cjs_require(caller, referrer, args)),
        NativeCallable::CjsRequireResolve { referrer } => {
            Some(call_cjs_require_resolve(caller, referrer, args))
        }
        NativeCallable::CjsRequireResolvePaths { referrer } => {
            Some(call_cjs_require_resolve_paths(caller, referrer, args))
        }
        NativeCallable::ImportMetaResolve { referrer } => {
            Some(call_import_meta_resolve(caller, referrer, args))
        }
        NativeCallable::CjsRequireCacheTrap { kind } => {
            Some(call_cjs_require_cache_trap(caller, kind, &args))
        }
        NativeCallable::PromiseResolvingFunction {
            promise,
            already_resolved,
            kind,
        } => {
            let mut already = already_resolved.lock().unwrap_or_else(|e| e.into_inner());
            if *already {
                return Some(value::encode_undefined());
            }
            *already = true;
            drop(already);
            match kind {
                PromiseResolvingKind::Fulfill => {
                    resolve_promise_from_caller(caller, promise, argument);
                }
                PromiseResolvingKind::Reject => {
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(argument));
                }
            }
            Some(value::encode_undefined())
        }
        NativeCallable::PromiseCombinatorReaction { .. } => Some(value::encode_undefined()),
        // 正常情况下被 drain loop 拦截，不会作为函数被调用；保险起见返回 undefined。
        NativeCallable::PromiseFinallyAwait { .. } => Some(value::encode_undefined()),
        NativeCallable::AsyncGeneratorIdentity { generator } => Some(generator),
        NativeCallable::GeneratorIdentity { generator } => Some(generator),
        NativeCallable::AsyncGeneratorMethod { generator, kind } => {
            let result_promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let request = AsyncGeneratorRequest {
                completion_type: kind,
                value: argument,
                promise: result_promise,
            };
            let completed = {
                let mut table = caller
                    .data()
                    .async_generator_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let Some(entry) = table.get_mut(value::decode_object_handle(generator) as usize)
                else {
                    return Some(result_promise);
                };
                if matches!(entry.state, AsyncGeneratorState::Completed) {
                    true
                } else {
                    entry.queue.push_back(request);
                    false
                }
            };
            if completed {
                match kind {
                    AsyncGeneratorCompletionType::Throw => settle_promise(
                        caller.data(),
                        result_promise,
                        PromiseSettlement::Reject(argument),
                    ),
                    _ => {
                        let result = alloc_iterator_result_from_caller(caller, argument, true);
                        resolve_promise_from_caller(caller, result_promise, result);
                    }
                }
            } else {
                pump_async_generator_from_caller(caller, generator);
            }
            Some(result_promise)
        }
        NativeCallable::IteratorProtoSymbolIterator => Some(this_val),
        NativeCallable::GeneratorMethod { .. } => Some(value::encode_undefined()),
        NativeCallable::MapSetMethod { kind } => Some(call_map_set_method_from_caller(
            caller, this_val, kind, args,
        )),
        NativeCallable::DateMethod { kind } => {
            Some(call_date_method_from_caller(caller, this_val, kind, args))
        }
        NativeCallable::WeakMapMethod { kind } => Some(call_weakmap_method_from_caller(
            caller, this_val, kind, args,
        )),
        NativeCallable::WeakSetMethod { kind } => Some(call_weakset_method_from_caller(
            caller, this_val, kind, args,
        )),
        NativeCallable::ProcessCwd => Some(crate::runtime_process::call_process_cwd(caller)),
        NativeCallable::ProcessExit => {
            Some(crate::runtime_process::call_process_exit(caller, &args))
        }
        NativeCallable::ProcessNextTick => Some(crate::runtime_process::call_process_next_tick(
            caller, &args,
        )),
        NativeCallable::ProcessStreamWrite { kind } => Some(
            crate::runtime_process::call_process_stream_write(caller, kind, &args),
        ),
        NativeCallable::ProcessEnvTrap { kind } => Some(
            crate::runtime_process::call_process_env_trap(caller, kind, &args),
        ),
        NativeCallable::ProcessStreamEnd { kind } => Some(
            crate::runtime_process::call_process_stream_end(caller, this_val, kind, &args),
        ),
        NativeCallable::ProcessStreamOn { kind } => Some(
            crate::runtime_process::call_process_stream_on(caller, this_val, kind, &args),
        ),
        NativeCallable::ProcessStdinResume => Some(
            crate::runtime_process::call_process_stdin_resume(caller, this_val),
        ),
        NativeCallable::ProcessHrtime => {
            Some(crate::runtime_process::call_process_hrtime(caller, &args))
        }
        NativeCallable::ProcessHrtimeBigint => {
            Some(crate::runtime_process::call_process_hrtime_bigint(caller))
        }
        NativeCallable::ProcessMemoryUsage => {
            Some(crate::runtime_process::call_process_memory_usage(caller))
        }
        NativeCallable::ProcessUptime => Some(crate::runtime_process::call_process_uptime(caller)),
        NativeCallable::ProcessCpuUsage => Some(crate::runtime_process::call_process_cpu_usage(
            caller, &args,
        )),
        NativeCallable::ProcessSend => Some(
            crate::runtime_node_child_process::call_child_process_method(
                caller,
                crate::runtime_node_child_process::ChildProcessMethodKind::ProcessSend,
                &args,
            ),
        ),
        NativeCallable::ProcessDisconnect => Some(
            crate::runtime_node_child_process::call_child_process_method(
                caller,
                crate::runtime_node_child_process::ChildProcessMethodKind::ProcessDisconnect,
                &args,
            ),
        ),
        NativeCallable::ProcessOn => Some(crate::runtime_process::call_process_on(caller, &args)),
        NativeCallable::FsMethod { kind } => {
            Some(crate::runtime_node_fs::call_fs_method(caller, kind, &args))
        }
        NativeCallable::CryptoMethod { kind } => Some(
            crate::runtime_node_crypto::call_crypto_method(caller, kind, &args),
        ),
        NativeCallable::ZlibMethod { kind } => Some(crate::runtime_node_zlib::call_zlib_method(
            caller, kind, &args,
        )),
        NativeCallable::ChildProcessMethod { kind } => {
            Some(crate::runtime_node_child_process::call_child_process_method(caller, kind, &args))
        }
        NativeCallable::NetMethod { kind } => Some(crate::runtime_node_net::call_net_method(
            caller, kind, &args,
        )),
        NativeCallable::VmMethod { kind } => {
            Some(crate::runtime_node_vm::call_vm_method(caller, kind, &args))
        }
        NativeCallable::AsyncHooksMethod { kind } => Some(
            crate::runtime_node_async_hooks::call_async_hooks_method(caller, kind, this_val, &args),
        ),
        NativeCallable::DgramMethod { kind } => Some(crate::runtime_node_dgram::call_dgram_method(
            caller, kind, &args,
        )),
        NativeCallable::TlsMethod { kind } => Some(crate::runtime_node_tls::call_tls_method(
            caller, kind, &args,
        )),
        NativeCallable::WorkerThreadsMethod { kind } => Some(
            crate::runtime_node_worker_threads::call_worker_threads_method(caller, kind, &args),
        ),
        NativeCallable::CryptoDigestMethod { state, kind } => {
            Some(crate::runtime_node_crypto::call_crypto_digest_method(
                caller, this_val, state, kind, &args,
            ))
        }
        NativeCallable::BufferConstructor => Some(crate::runtime_buffer::call_buffer_constructor(
            caller, &args,
        )),
        NativeCallable::BufferStatic { kind } => Some(crate::runtime_buffer::call_buffer_static(
            caller, kind, &args,
        )),
        NativeCallable::BufferMethod { kind } => Some(crate::runtime_buffer::call_buffer_method(
            caller, this_val, kind, &args,
        )),
        NativeCallable::TextEncoderConstructor => Some(
            crate::runtime_node_globals::call_text_encoder_constructor(caller),
        ),
        NativeCallable::TextEncoderMethod { kind } => Some(
            crate::runtime_node_globals::call_text_encoder_method(caller, kind, &args),
        ),
        NativeCallable::TextDecoderConstructor => Some(
            crate::runtime_node_globals::call_text_decoder_constructor(caller, &args),
        ),
        NativeCallable::TextDecoderMethod { kind } => Some(
            crate::runtime_node_globals::call_text_decoder_method(caller, this_val, kind, &args),
        ),
        NativeCallable::StructuredClone => Some(crate::runtime_structured_clone::structured_clone(
            caller, &args,
        )),
        NativeCallable::Atob => Some(crate::runtime_node_globals::call_atob(caller, &args)),
        NativeCallable::Btoa => Some(crate::runtime_node_globals::call_btoa(caller, &args)),
        NativeCallable::QueueMicrotask => Some(crate::runtime_node_globals::call_queue_microtask(
            caller, &args,
        )),
        NativeCallable::PerformanceNow => {
            Some(crate::runtime_node_globals::call_performance_now(caller))
        }
        NativeCallable::PerfHooksMethod { kind } => Some(
            crate::runtime_node_perf_hooks::call_perf_hooks_method(caller, kind, &args),
        ),
        NativeCallable::OsInfo { kind } => {
            Some(crate::runtime_node_globals::call_os_info(caller, kind))
        }
        NativeCallable::ArrayConstructor => {
            // `new Array(n)` / ArraySpeciesCreate: capacity+length from first arg when it is a
            // single finite length number. Element writes grow as needed either way.
            if value::is_object(this_val) && !value::is_array(this_val) {
                Some(this_val)
            } else {
                let length = if args.len() == 1 && value::is_f64(argument) {
                    let n = value::decode_f64(argument);
                    if n.is_finite() && n >= 0.0 && n == n.trunc() && n <= u32::MAX as f64 {
                        n as u32
                    } else {
                        0
                    }
                } else {
                    args.len() as u32
                };
                let arr = alloc_array(caller, length.max(args.len() as u32));
                if let Some(ptr) = resolve_array_ptr(caller, arr) {
                    if args.len() == 1 && value::is_f64(argument) {
                        // length-only construction: empty slots are holes up to length
                        for i in 0..length {
                            write_array_hole(caller, ptr, i);
                        }
                        write_array_length(caller, ptr, length);
                    } else {
                        for (i, val) in args.iter().copied().enumerate() {
                            write_array_elem(caller, ptr, i as u32, val);
                        }
                        write_array_length(caller, ptr, args.len() as u32);
                    }
                }
                Some(arr)
            }
        }
        NativeCallable::ObjectConstructor => {
            if value::is_object(this_val) || value::is_function(this_val) {
                Some(this_val)
            } else {
                Some({
                    let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
                    alloc_host_object(caller, &_wjsm_env, 4)
                })
            }
        }
        NativeCallable::ErrorProtoToString => Some(error_proto_to_string_impl(caller, this_val)),
        NativeCallable::ObjectProtoToString => Some(obj_proto_to_string_impl(caller, this_val)),
        NativeCallable::ObjectProtoValueOf => Some(this_val),
        NativeCallable::FunctionProtoCall
        | NativeCallable::FunctionProtoApply
        | NativeCallable::FunctionProtoBind => {
            // call/apply 需要 shadow stack / reentry，走 async 分派。
            None
        }
        NativeCallable::StringConstructor => {
            let arg = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if value::is_undefined(arg) {
                Some(store_runtime_string(caller, String::new()))
            } else if value::is_symbol(arg) {
                Some(symbol_proto_to_string_impl(caller, arg))
            } else {
                let s = render_value(caller, arg).unwrap_or_default();
                Some(store_runtime_string(caller, s))
            }
        }
        NativeCallable::BooleanConstructor
        | NativeCallable::NumberConstructor
        | NativeCallable::BigIntConstructor => Some(value::encode_undefined()),
        // Function 构造器在 async 分派中实现（需解析 body 并建 EvalFunction）
        NativeCallable::RegExpConstructor => Some(regexp_constructor_impl(caller, this_val, &args)),
        NativeCallable::SymbolConstructor => Some({
            let desc = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let description = if value::is_undefined(desc) {
                None
            } else if value::is_string(desc) {
                Some(get_string_utf8_lossy(caller, desc))
            } else {
                Some(
                    render_value(caller, desc)
                        .unwrap_or_default()
                        .trim_matches('"')
                        .to_string(),
                )
            };
            let mut table = caller
                .data()
                .symbol_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let handle = table.len() as u32;
            table.push(SymbolEntry {
                description,
                global_key: None,
            });
            value::encode_symbol_handle(handle)
        }),
        NativeCallable::ErrorConstructor
        | NativeCallable::TypeErrorConstructor
        | NativeCallable::RangeErrorConstructor
        | NativeCallable::SyntaxErrorConstructor
        | NativeCallable::ReferenceErrorConstructor
        | NativeCallable::URIErrorConstructor
        | NativeCallable::EvalErrorConstructor
        | NativeCallable::AggregateErrorConstructor => {
            let error_name = match &record {
                NativeCallable::ErrorConstructor => "Error",
                NativeCallable::TypeErrorConstructor => "TypeError",
                NativeCallable::RangeErrorConstructor => "RangeError",
                NativeCallable::SyntaxErrorConstructor => "SyntaxError",
                NativeCallable::ReferenceErrorConstructor => "ReferenceError",
                NativeCallable::URIErrorConstructor => "URIError",
                NativeCallable::EvalErrorConstructor => "EvalError",
                NativeCallable::AggregateErrorConstructor => "AggregateError",
                _ => "Error",
            };
            let msg = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            Some(create_error_object_with_receiver(
                caller, error_name, msg, options, this_val,
            ))
        }
        NativeCallable::MapConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::SetConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::WeakMapConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::WeakSetConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::WeakRefConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::FinalizationRegistryConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::DateConstructorGlobal => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        NativeCallable::PromiseConstructor => {
            Some(alloc_promise_from_caller(caller, PromiseEntry::pending()))
        }
        NativeCallable::ArrayBufferConstructorGlobal => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        NativeCallable::DataViewConstructorGlobal => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        NativeCallable::TypedArrayConstructor(kind) => {
            let (element_size, element_kind) = kind.element();
            Some(typedarray_construct(
                caller,
                argument,
                args.get(1).copied().unwrap_or_else(value::encode_undefined),
                args.get(2).copied().unwrap_or_else(value::encode_undefined),
                element_size,
                element_kind,
                Some(this_val),
            ))
        }
        NativeCallable::BigInt64ArrayConstructor => Some(typedarray_construct(
            caller,
            argument,
            args.get(1).copied().unwrap_or_else(value::encode_undefined),
            args.get(2).copied().unwrap_or_else(value::encode_undefined),
            8,
            4,
            Some(this_val),
        )),
        NativeCallable::BigUint64ArrayConstructor => Some(typedarray_construct(
            caller,
            argument,
            args.get(1).copied().unwrap_or_else(value::encode_undefined),
            args.get(2).copied().unwrap_or_else(value::encode_undefined),
            8,
            5,
            Some(this_val),
        )),
        NativeCallable::ProxyConstructor => Some({
            let target = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let handler = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            if !value::is_js_object(target) {
                make_type_error_exception(caller, "TypeError: Proxy target must be an object")
            } else if !value::is_js_object(handler) {
                make_type_error_exception(caller, "TypeError: Proxy handler must be an object")
            } else {
                let handle = {
                    let mut table = caller
                        .data()
                        .proxy_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let handle = table.len() as u32;
                    table.push(ProxyEntry {
                        target,
                        handler,
                        revoked: false,
                    });
                    handle
                };
                value::encode_proxy_handle(handle)
            }
        }),
        NativeCallable::ProxyRevoker { proxy_handle } => {
            let mut table = caller
                .data()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = table.get_mut(proxy_handle as usize) {
                entry.revoked = true;
            }
            Some(value::encode_undefined())
        }
        NativeCallable::WeakRefDerefMethod => Some(weakref_deref_impl(caller, this_val)),
        NativeCallable::FinalizationRegistryRegisterMethod => {
            Some(fr_register_impl_with_args(caller, this_val, args))
        }
        NativeCallable::FinalizationRegistryUnregisterMethod => {
            let token = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            Some(fr_unregister_impl(caller, this_val, token))
        }
        NativeCallable::StubGlobal(_) => Some(value::encode_undefined()),
        NativeCallable::GcCollect => {
            let env = crate::wasm_env::WasmEnv::from_caller(&mut *caller).expect("WasmEnv");
            let algorithm = caller.data().gc_algorithm.as_str();
            let stats =
                crate::runtime_gc::active_zgc::collect_dispatch(&mut *caller, &env, algorithm);
            caller
                .data()
                .performance_forced_gc
                .store(true, std::sync::atomic::Ordering::Release);
            caller.data().store_last_gc_stats(algorithm, stats);
            Some(value::encode_undefined())
        }
        NativeCallable::SharedArrayBufferConstructor => {
            let length = argument;
            let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            Some(crate::shared_buffer::construct_shared_array_buffer(
                caller, length, options, this_val,
            ))
        }
        // ── Agent harness ──
        NativeCallable::AgentStart => {
            let script = argument;
            crate::agent_cluster::agent_start(caller, script)
        }
        NativeCallable::AgentBroadcast => {
            let sab = argument;
            crate::agent_cluster::agent_broadcast(caller, sab)
        }
        NativeCallable::AgentReceiveBroadcast => {
            crate::agent_cluster::agent_receive_broadcast(caller, argument)
        }
        NativeCallable::AgentReport => {
            let msg = argument;
            crate::agent_cluster::agent_report(caller, msg)
        }
        NativeCallable::AgentGetReport => {
            let shared = match caller.data().shared_state.clone() {
                Some(s) => s,
                None => return Some(value::encode_undefined()),
            };
            let report = shared
                .agent_state
                .reports
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop();
            match report {
                Some(r) => Some(store_runtime_string(caller, r)),
                None => Some(value::encode_null()),
            }
        }
        NativeCallable::AgentSleep => {
            let ms = args.first().copied().map(value::decode_f64).unwrap_or(0.0) as u64;
            std::thread::sleep(std::time::Duration::from_millis(ms));
            Some(value::encode_undefined())
        }
        NativeCallable::AgentMonotonicNow => {
            static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            let start = START.get_or_init(std::time::Instant::now);
            Some(value::encode_f64(start.elapsed().as_millis() as f64))
        }
        NativeCallable::AtomicsGlobal => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        // ── Async iterator methods ──
        NativeCallable::AsyncIteratorProtoSymbolAsyncIterator => Some(this_val),
        NativeCallable::AsyncFromSyncNext { handle: _ } => {
            // 仅由 iterator_next_async → call_native_callable_with_args_from_caller_async 调用
            Some(value::encode_undefined())
        }
        NativeCallable::AsyncFromSyncReturn { handle: _ } => {
            // 仅由 async 路径（call_native_callable_with_args_from_caller_async）处理
            Some(value::encode_undefined())
        }
        NativeCallable::AsyncFromSyncThrow { handle } => {
            let arg = args.first().copied().unwrap_or(value::encode_undefined());
            let (_sync_iter_handle, sync_done) = {
                let table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let entry = match table.get(handle as usize) {
                    Some(e) => e,
                    None => return Some(value::encode_undefined()),
                };
                (entry.sync_iterator, entry.sync_done)
            };
            if sync_done {
                let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(arg));
                return Some(promise);
            }
            {
                let mut table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.sync_done = true;
                }
            }
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            settle_promise(caller.data(), promise, PromiseSettlement::Reject(arg));
            Some(promise)
        }
        NativeCallable::HeadersMethod { kind, .. } => {
            call_headers_method_from_caller(caller, this_val, kind, &args)
        }
        NativeCallable::ResponseMethod { kind, .. } => {
            call_response_method_from_caller(caller, this_val, kind, &args)
        }
        NativeCallable::RequestMethod { kind, .. } => {
            call_request_method_from_caller(caller, this_val, kind, &args)
        }
        NativeCallable::HeadersConstructor => construct_headers(caller, this_val, &args),
        NativeCallable::ResponseConstructor => construct_response(caller, this_val, &args),
        NativeCallable::RequestConstructor => construct_request(caller, this_val, &args),
        NativeCallable::AbortControllerConstructor => {
            construct_abort_controller(caller, this_val, &args)
        }
        NativeCallable::AbortControllerAbort { signal_handle } => {
            abort_controller_abort(caller, signal_handle, &args)
        }
        // ── ReadableStream (WHATWG Streams Phase 1) ──
        // ReadableStreamConstructor is async-only: routed through the host-import
        // `readable_stream_constructor` (linker.func_wrap_async in fetch.rs). It is
        // never dispatched via the sync NativeCallable path.
        NativeCallable::ReadableStreamConstructor => Some(value::encode_undefined()),
        NativeCallable::ReadableStreamMethod { handle, kind } => {
            call_readable_stream_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::ReadableStreamDefaultReaderMethod { handle, kind } => {
            call_default_reader_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::ReadableStreamDefaultControllerMethod { handle, kind } => {
            call_default_controller_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::ReadableStreamByobRequestMethod { handle, kind } => {
            call_byob_request_method_from_caller(caller, this_val, handle, kind, &args)
        }
        // ── ReadableStream async iterator (WHATWG Streams Phase 2) ──
        NativeCallable::ReadableStreamAsyncIteratorNext { reader_handle } => {
            call_default_reader_method_from_caller(
                caller,
                this_val,
                reader_handle,
                ReadableStreamDefaultReaderMethodKind::Read,
                &args,
            )
        }
        NativeCallable::ReadableStreamPipeToWriteFulfilled { readable_handle } => {
            Some(finish_pipe_to_write(caller, readable_handle, None))
        }
        NativeCallable::ReadableStreamPipeToWriteRejected { readable_handle } => Some(
            finish_pipe_to_write(caller, readable_handle, Some(argument)),
        ),
        NativeCallable::ReadableStreamAsyncIteratorReturn { reader_handle } => {
            // releaseLock：释放流的锁定
            let stream_handle = {
                let reader_table = caller
                    .data()
                    .reader_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                reader_table
                    .get(reader_handle as usize)
                    .map(|e| e.stream_handle)
            };
            if let Some(sh) = stream_handle {
                let mut stream_table = caller
                    .data()
                    .readable_stream_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = stream_table.get_mut(sh as usize) {
                    entry.locked = false;
                }
            }
            // 返回 {done: true, value: undefined} 作为 resolved Promise
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let result = build_reader_result(caller, true, None);
            settle_promise(caller.data(), p, PromiseSettlement::Fulfill(result));
            Some(p)
        }
        // ── WritableStream (WHATWG Streams Phase 4) ──
        // WritableStreamConstructor is async-only: routed through the host-import
        // `writable_stream_constructor` (linker.func_wrap_async in fetch.rs). It is
        // never dispatched via the sync NativeCallable path.
        NativeCallable::WritableStreamConstructor => Some(value::encode_undefined()),
        NativeCallable::WritableStreamMethod { handle, kind } => {
            call_writable_stream_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::WritableStreamDefaultWriterMethod { handle, kind } => {
            call_default_writer_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::WritableStreamDefaultControllerMethod { handle, kind } => {
            call_writable_controller_method_from_caller(caller, this_val, handle, kind, &args)
        }
        // ── TransformStream (WHATWG Streams Phase 5) ──
        // TransformStreamConstructor is async-only: routed through the host-import
        // `transform_stream_constructor`. It is never dispatched via the sync NativeCallable path.
        NativeCallable::TransformStreamConstructor => Some(value::encode_undefined()),
        NativeCallable::TransformStreamMethod { handle, kind } => {
            call_transform_stream_method_from_caller(caller, this_val, handle, kind, &args)
        }
        // ── QueuingStrategy (WHATWG Streams Phase 2) ──
        NativeCallable::CountQueuingStrategyConstructor => {
            construct_count_queuing_strategy(caller, this_val, &args)
        }
        NativeCallable::ByteLengthQueuingStrategyConstructor => {
            construct_byte_length_queuing_strategy(caller, this_val, &args)
        }
        NativeCallable::QueuingStrategySize { kind } => {
            call_queuing_strategy_size_from_caller(caller, kind, &args)
        }
        NativeCallable::ObjectStatic { .. } | NativeCallable::PromiseStatic { .. } => {
            // 静态方法需 async host（Object.keys 等）；sync 路径返回 undefined 并记错误
            set_runtime_error(
                caller.data(),
                "Object/Promise static method requires async NativeCallable path".to_string(),
            );
            Some(value::encode_undefined())
        }
    }
}

pub(crate) async fn call_native_callable_with_args_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    callable: i64,
    this_val: i64,
    args: Vec<i64>,
) -> Option<i64> {
    if !value::is_native_callable(callable) {
        return None;
    }

    let idx = value::decode_native_callable_idx(callable) as usize;
    let record = {
        let table = caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table.get(idx).cloned()
    }?;
    let argument = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);

    match record {
        NativeCallable::ArgumentsStrictCalleeGetter => Some(
            crate::runtime_arguments::arguments_strict_callee_getter(caller, this_val),
        ),
        NativeCallable::FunctionConstructor => {
            Some(function_constructor_impl_async(caller, &args).await)
        }
        NativeCallable::ObjectStatic { kind } => {
            Some(call_object_static_async(caller, kind, &args).await)
        }
        NativeCallable::PromiseStatic { kind } => {
            Some(call_promise_static_async(caller, kind, &args).await)
        }

        NativeCallable::EvalIndirect => {
            if !crate::runtime_node_vm::current_realm_allows_string_codegen(caller) {
                return Some(make_eval_error_exception(
                    caller,
                    "EvalError: Code generation from strings disallowed for this context",
                ));
            }
            Some(perform_eval_from_caller_async(caller, argument, None).await)
        }

        NativeCallable::EvalFunction(function) => {
            Some(call_eval_function_from_caller_async(caller, function, args).await)
        }
        NativeCallable::AgentReceiveBroadcast => {
            crate::agent_cluster::agent_receive_broadcast_async(caller, argument).await
        }
        NativeCallable::AgentReport => crate::agent_cluster::agent_report(caller, argument),
        NativeCallable::AgentGetReport => {
            let shared = match caller.data().shared_state.clone() {
                Some(s) => s,
                None => return Some(value::encode_undefined()),
            };
            let report = shared
                .agent_state
                .reports
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop();
            match report {
                Some(r) => Some(store_runtime_string(caller, r)),
                None => Some(value::encode_null()),
            }
        }
        NativeCallable::AgentSleep => {
            let ms = args.first().copied().map(value::decode_f64).unwrap_or(0.0) as u64;
            std::thread::sleep(std::time::Duration::from_millis(ms));
            Some(value::encode_undefined())
        }
        NativeCallable::AsyncFromSyncNext { handle } => {
            Some(Box::pin(advance_async_from_sync_async(caller, handle)).await)
        }
        NativeCallable::AsyncFromSyncReturn { handle } => {
            let arg = args.first().copied().unwrap_or(value::encode_undefined());
            let (sync_iter_handle, sync_done) = {
                let table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let entry = match table.get(handle as usize) {
                    Some(e) => e,
                    None => return Some(value::encode_undefined()),
                };
                (entry.sync_iterator, entry.sync_done)
            };
            if sync_done {
                let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
                let result = alloc_iterator_result_from_caller(caller, arg, true);
                resolve_promise_from_caller(caller, promise, result);
                return Some(promise);
            }
            {
                let mut table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.sync_done = true;
                }
            }
            Some(call_sync_iter_and_wrap_async(caller, sync_iter_handle, Some(arg), false).await)
        }
        NativeCallable::RegExpPrimitiveMethod { method } => {
            Some(invoke_regexp_primitive_method_async(caller, this_val, method, &args).await)
        }
        NativeCallable::FunctionProtoCall => {
            let this_arg = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let call_args = if args.len() > 1 { &args[1..] } else { &[] };
            Some(
                crate::host_imports::reflect_apply_impl_async(
                    caller, this_val, this_arg, call_args,
                )
                .await,
            )
        }
        NativeCallable::FunctionProtoApply => {
            let this_arg = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let arr_like = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            let call_args =
                match crate::host_imports::extract_array_like_elements(caller, arr_like).await {
                    Ok(v) => v,
                    Err(msg) => {
                        return Some(make_type_error_exception(
                            caller,
                            &format!("TypeError: {msg}"),
                        ));
                    }
                };
            Some(
                crate::host_imports::reflect_apply_impl_async(
                    caller, this_val, this_arg, &call_args,
                )
                .await,
            )
        }
        NativeCallable::FunctionProtoBind => {
            let this_arg = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let bound_args = if args.len() > 1 {
                args[1..].to_vec()
            } else {
                Vec::new()
            };
            let mut bound = caller
                .data()
                .bound_objects
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let idx = bound.len() as u32;
            bound.push(BoundRecord {
                target_func: this_val,
                bound_this: this_arg,
                bound_args,
            });
            Some(value::encode_bound_idx(idx))
        }
        NativeCallable::RegExpStringIteratorNext { iter_handle } => Some(
            crate::runtime_regexp::regexp_string_iterator_step(caller, iter_handle),
        ),
        NativeCallable::RegExpStringIteratorSelf => Some(this_val),
        NativeCallable::MapSetMethod {
            kind: MapSetMethodKind::ForEach,
        } => Some(map_set_for_each_impl_async(caller, this_val, &args).await),
        NativeCallable::HeadersMethod { kind, .. } => {
            call_headers_method_from_caller_async(caller, this_val, kind, &args).await
        }
        NativeCallable::GeneratorMethod { generator, kind } => {
            Some(call_generator_method_from_caller_async(caller, generator, kind, argument).await)
        }
        NativeCallable::CjsRequire { referrer } => {
            Some(call_cjs_require_async(caller, referrer, args).await)
        }
        NativeCallable::VmMethod { kind } => {
            Some(crate::runtime_node_vm::call_vm_method_async(caller, kind, &args).await)
        }
        NativeCallable::ProcessNextTick => {
            Some(crate::runtime_process::call_process_next_tick_async(caller, &args).await)
        }
        NativeCallable::AsyncHooksMethod { kind } => Some(
            crate::runtime_node_async_hooks::call_async_hooks_method_async(
                caller, kind, this_val, &args,
            )
            .await,
        ),
        NativeCallable::AgentStart
        | NativeCallable::AgentBroadcast
        | NativeCallable::AgentMonotonicNow => {
            call_native_callable_with_args_from_caller(caller, callable, this_val, args)
        }
        _ => call_native_callable_with_args_from_caller(caller, callable, this_val, args),
    }
}
/// 创建 AsyncFromSyncIterator：将同步迭代器包装为异步迭代器协议。
/// 在 iterators 表中注册 ObjectIter（next/return 为 NativeCallable），
/// 返回 TAG_ITERATOR 句柄供 for-await 使用。
pub(crate) fn create_async_from_sync_iterator(
    caller: &mut Caller<'_, RuntimeState>,
    sync_iter_handle: i64,
) -> i64 {
    let mut iters = caller
        .data()
        .iterators
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let iter_handle = iters.len() as u32;
    let outer_iter = value::encode_handle(value::TAG_ITERATOR, iter_handle);

    let table_idx = {
        let mut table = caller
            .data()
            .async_from_sync_iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let idx = table.len() as u32;
        table.push(AsyncFromSyncIteratorEntry {
            sync_iterator: sync_iter_handle,
            sync_done: false,
            outer_iter,
            outer_handle_idx: iter_handle,
        });
        idx
    };

    let next_callable = {
        let mut nc = caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = nc.len() as u32;
        nc.push(NativeCallable::AsyncFromSyncNext { handle: table_idx });
        value::encode_native_callable_idx(handle)
    };
    let return_callable = {
        let mut nc = caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = nc.len() as u32;
        nc.push(NativeCallable::AsyncFromSyncReturn { handle: table_idx });
        value::encode_native_callable_idx(handle)
    };

    iters.push(IteratorState::ObjectIter {
        iterator: sync_iter_handle,
        next: next_callable,
        return_method: Some(return_callable),
        throw_method: None,
        current_value: value::encode_undefined(),
        has_current: false,
        done: false,
    });
    outer_iter
}

/// 调用同步迭代器的方法并将结果包装为 resolved Promise。
async fn call_sync_iter_and_wrap_async(
    caller: &mut Caller<'_, RuntimeState>,
    sync_iter_handle: i64,
    arg_if_return: Option<i64>,
    is_throw: bool,
) -> i64 {
    let sync_handle_idx = value::decode_handle(sync_iter_handle) as usize;

    let (iterator, method_to_call) = {
        let iters = caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match iters.get(sync_handle_idx) {
            Some(IteratorState::ObjectIter {
                iterator,
                next,
                return_method,
                ..
            }) => {
                let method = if arg_if_return.is_some() {
                    return_method.unwrap_or(*next)
                } else {
                    *next
                };
                (*iterator, method)
            }
            _ => return value::encode_undefined(),
        }
    };

    if is_throw {
        let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        settle_promise(
            caller.data(),
            promise,
            PromiseSettlement::Reject(arg_if_return.unwrap_or(value::encode_undefined())),
        );
        return promise;
    }

    let call_arg = arg_if_return.unwrap_or(value::encode_undefined());
    let raw_result = call_iterator_method_async(caller, method_to_call, iterator, call_arg).await;

    if value::is_exception(raw_result) {
        {
            let mut iters = caller
                .data()
                .iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(IteratorState::ObjectIter { done, .. }) = iters.get_mut(sync_handle_idx) {
                *done = true;
            }
        }
        let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        let reason = exception_reason(caller, raw_result);
        settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
        return promise;
    }

    let (done, current_value) = if (value::is_object(raw_result)
        || value::is_function(raw_result)
        || value::is_array(raw_result))
        && let Some(ptr) = resolve_handle(caller, raw_result)
    {
        let done = read_object_property_by_name(caller, ptr, "done")
            .map(nanbox_to_bool)
            .unwrap_or(true);
        let value =
            read_object_property_by_name(caller, ptr, "value").unwrap_or(value::encode_undefined());
        (done, value)
    } else {
        (true, value::encode_undefined())
    };

    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let result = alloc_iterator_result_from_caller(caller, current_value, done);
    resolve_promise_from_caller(caller, promise, result);
    promise
}

/// AsyncFromSyncIterator.next()：推进同步迭代器并返回 Promise<IteratorResult>。
pub(crate) async fn advance_async_from_sync_async(
    caller: &mut Caller<'_, RuntimeState>,
    handle: u32,
) -> i64 {
    let (sync_iter_handle, sync_done) = {
        let table = caller
            .data()
            .async_from_sync_iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = match table.get(handle as usize) {
            Some(e) => e,
            None => return value::encode_undefined(),
        };
        (entry.sync_iterator, entry.sync_done)
    };

    if sync_done {
        let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        let result = alloc_iterator_result_from_caller(caller, value::encode_undefined(), true);
        resolve_promise_from_caller(caller, promise, result);
        return promise;
    }
    let sync_handle_idx = value::decode_handle(sync_iter_handle) as usize;

    // Direct advancement for non-ObjectIter types
    let direct_result = {
        let mut iters = caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match iters.get_mut(sync_handle_idx) {
            Some(IteratorState::ArrayIter { ptr, index, length }) => {
                if *index < *length {
                    let idx = *index;
                    let array_ptr = *ptr;
                    *index += 1;
                    drop(iters);
                    let val = read_array_elem(caller, array_ptr, idx)
                        .unwrap_or(value::encode_undefined());
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::MapKeyIter {
                map_handle, index, ..
            }) => {
                let table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if *map_handle < table.len() as u32 {
                    let entry = &table[*map_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.keys.len() {
                        let val = entry.keys[idx];
                        *index += 1;
                        drop(table);
                        Some((false, val))
                    } else {
                        drop(table);
                        Some((true, value::encode_undefined()))
                    }
                } else {
                    drop(table);
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::MapValueIter {
                map_handle, index, ..
            }) => {
                let table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if *map_handle < table.len() as u32 {
                    let entry = &table[*map_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.values.len() {
                        let val = entry.values[idx];
                        *index += 1;
                        drop(table);
                        Some((false, val))
                    } else {
                        drop(table);
                        Some((true, value::encode_undefined()))
                    }
                } else {
                    drop(table);
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::MapEntryIter {
                map_handle, index, ..
            }) => {
                let table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if *map_handle < table.len() as u32 {
                    let entry = &table[*map_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.keys.len() {
                        let key = entry.keys[idx];
                        let item = entry.values[idx];
                        *index += 1;
                        drop(table);
                        drop(iters);
                        let pair = alloc_array(caller, 2);
                        if let Some(pair_ptr) = resolve_array_ptr(caller, pair) {
                            write_array_elem(caller, pair_ptr, 0, key);
                            write_array_elem(caller, pair_ptr, 1, item);
                            write_array_length(caller, pair_ptr, 2);
                        }
                        Some((false, pair))
                    } else {
                        drop(table);
                        Some((true, value::encode_undefined()))
                    }
                } else {
                    drop(table);
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::SetValueIter {
                set_handle, index, ..
            }) => {
                let table = caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if *set_handle < table.len() as u32 {
                    let entry = &table[*set_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.values.len() {
                        let val = entry.values[idx];
                        *index += 1;
                        drop(table);
                        Some((false, val))
                    } else {
                        drop(table);
                        Some((true, value::encode_undefined()))
                    }
                } else {
                    drop(table);
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::SetEntryIter {
                set_handle, index, ..
            }) => {
                let table = caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if *set_handle < table.len() as u32 {
                    let entry = &table[*set_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.values.len() {
                        let item = entry.values[idx];
                        *index += 1;
                        drop(table);
                        drop(iters);
                        let pair = alloc_array(caller, 2);
                        if let Some(pair_ptr) = resolve_array_ptr(caller, pair) {
                            write_array_elem(caller, pair_ptr, 0, item);
                            write_array_elem(caller, pair_ptr, 1, item);
                            write_array_length(caller, pair_ptr, 2);
                        }
                        Some((false, pair))
                    } else {
                        drop(table);
                        Some((true, value::encode_undefined()))
                    }
                } else {
                    drop(table);
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::IndexValueIter { values, index }) => {
                if (*index as usize) < values.len() {
                    let val = values[*index as usize];
                    *index += 1;
                    drop(iters);
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::TypedArrayValueIter {
                entry,
                index,
                length,
            }) => {
                if *index < *length {
                    let entry = entry.clone();
                    let idx = *index;
                    *index += 1;
                    drop(iters);
                    let val = typedarray_element_read_entry(caller, &entry, idx)
                        .unwrap_or(value::encode_undefined());
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::TypedArrayEntryIter {
                entry,
                index,
                length,
            }) => {
                if *index < *length {
                    let typedarray_entry = entry.clone();
                    let idx = *index;
                    *index += 1;
                    drop(iters);
                    let entry = alloc_array(caller, 2);
                    if let Some(entry_ptr) = resolve_array_ptr(caller, entry) {
                        let elem = typedarray_element_read_entry(caller, &typedarray_entry, idx)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(caller, entry_ptr, 0, value::encode_f64(idx as f64));
                        write_array_elem(caller, entry_ptr, 1, elem);
                        write_array_length(caller, entry_ptr, 2);
                    }
                    Some((false, entry))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::RegExpStringIter { .. }) => {
                drop(iters);
                let done = regexp_string_iter_ensure_current(caller, sync_handle_idx);
                if done {
                    Some((true, value::encode_undefined()))
                } else {
                    let val = regexp_string_iter_value(caller, sync_handle_idx);
                    regexp_string_iter_next(caller, sync_handle_idx);
                    Some((false, val))
                }
            }
            Some(IteratorState::StringIter { string, unit_pos }) => {
                if *unit_pos < string.utf16_len() {
                    let pos = *unit_pos;
                    let string = string.clone();
                    string_iter_advance_unit_pos(&string, unit_pos);
                    drop(iters);
                    let val = string_iter_current_value(caller, &string, pos);
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            _ => None,
        }
    };

    if let Some((done, current_value)) = direct_result {
        if done {
            let mut table = caller
                .data()
                .async_from_sync_iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = table.get_mut(handle as usize) {
                entry.sync_done = true;
            }
        }
        let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        let result = alloc_iterator_result_from_caller(caller, current_value, done);
        resolve_promise_from_caller(caller, promise, result);
        return promise;
    }

    let promise = call_sync_iter_and_wrap_async(caller, sync_iter_handle, None, false).await;

    {
        let iters = caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(IteratorState::ObjectIter { done, .. }) = iters.get(sync_handle_idx)
            && *done
        {
            drop(iters);
            let mut table = caller
                .data()
                .async_from_sync_iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = table.get_mut(handle as usize) {
                entry.sync_done = true;
            }
        }
    }

    promise
}
pub(crate) fn weakref_deref_impl(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> i64 {
    if !value::is_object(this_val) {
        return value::encode_undefined();
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let handle_val =
        obj_ptr.and_then(|p| read_object_property_by_name(caller, p, "__weakref_handle__"));
    let handle = handle_val
        .map(|v| value::decode_f64(v) as usize)
        .unwrap_or(0);
    let table = caller
        .data()
        .weakref_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if handle >= table.len() {
        return value::encode_undefined();
    }
    let Some(target_handle) = table[handle].target_handle else {
        return value::encode_undefined();
    };
    drop(table);
    if !obj_table_handle_live(caller, target_handle) {
        return value::encode_undefined();
    }
    encode_handle_as_js_value(caller, target_handle).unwrap_or_else(value::encode_undefined)
}
pub(crate) fn fr_unregister_impl(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    token: i64,
) -> i64 {
    if !value::is_object(this_val) {
        return value::encode_bool(false);
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let handle_val = obj_ptr
        .and_then(|p| read_object_property_by_name(caller, p, "__finalization_registry_handle__"));
    let Some(handle) = handle_val.map(|v| value::decode_f64(v) as usize) else {
        return value::encode_bool(false);
    };
    let mut table = caller
        .data()
        .finalization_registry_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if handle >= table.len() {
        return value::encode_bool(false);
    }
    let entry = &mut table[handle];
    let initial_len = entry.registrations.len();
    entry.registrations.retain(|r| match &r.unregister_token {
        Some(t) => !same_value_zero(caller, *t, token),
        None => true,
    });
    value::encode_bool(entry.registrations.len() < initial_len)
}
pub(crate) fn fr_register_impl_with_args(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: Vec<i64>,
) -> i64 {
    if args.len() < 2 {
        return value::encode_undefined();
    }
    let target = args[0];
    let held_value = args[1];
    let unregister_token = if args.len() >= 3 {
        let token = args[2];
        if value::is_js_object(token) || value::is_symbol(token) {
            Some(token)
        } else {
            None
        }
    } else {
        None
    };
    if !value::is_js_object(target) {
        return value::encode_undefined();
    }
    let target_handle = match weak_target_handle_index_of(caller, target) {
        Some(h) => h,
        None => return value::encode_undefined(),
    };
    if !value::is_object(this_val) {
        return value::encode_undefined();
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let handle_val = obj_ptr
        .and_then(|p| read_object_property_by_name(caller, p, "__finalization_registry_handle__"));
    let Some(handle) = handle_val.map(|v| value::decode_f64(v) as usize) else {
        return value::encode_undefined();
    };
    {
        let mut table = caller
            .data()
            .finalization_registry_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if handle < table.len() {
            table[handle].registrations.push(FinalizationRegistration {
                target_handle,
                held_value,
                unregister_token,
            });
        }
    }
    value::encode_undefined()
}

/// `new Function(...args, body)` / `Function(...args, body)`。
///
/// 受当前 execution_realm 的 `codeGeneration.strings` 约束；false 时抛 EvalError。
async fn function_constructor_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    args: &[i64],
) -> i64 {
    if !crate::runtime_node_vm::current_realm_allows_string_codegen(caller) {
        return make_eval_error_exception(
            caller,
            "EvalError: Code generation from strings disallowed for this context",
        );
    }

    let (param_names, body_src) = split_function_ctor_args(caller, args);

    for name in &param_names {
        if name.is_empty() || !is_simple_js_ident(name) {
            return make_syntax_error_exception(
                caller,
                "SyntaxError: Unexpected identifier in Function parameter list",
            );
        }
    }

    let body_stmts = match parse_function_body_as_stmts(&body_src) {
        Ok(s) => s,
        Err(e) => {
            return make_syntax_error_exception(caller, &format!("SyntaxError: {e}"));
        }
    };

    let function = EvalFunction {
        params: param_names,
        body: body_stmts,
        scope_env: None,
    };
    create_eval_function(caller.data(), function)
}

fn split_function_ctor_args(
    caller: &mut Caller<'_, RuntimeState>,
    args: &[i64],
) -> (Vec<String>, String) {
    if args.is_empty() {
        return (Vec::new(), String::new());
    }
    let body = function_ctor_arg_to_string(caller, *args.last().unwrap());
    let mut params = Vec::with_capacity(args.len().saturating_sub(1));
    for arg in &args[..args.len() - 1] {
        params.push(function_ctor_arg_to_string(caller, *arg));
    }
    (params, body)
}

fn function_ctor_arg_to_string(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    if value::is_string(val) {
        get_string_utf8_lossy(caller, val)
    } else {
        render_value(caller, val).unwrap_or_default()
    }
}

fn is_simple_js_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c == '$' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric())
}

fn parse_function_body_as_stmts(code: &str) -> Result<Vec<swc_core::ecma::ast::Stmt>, String> {
    let module = wjsm_parser::parse_script_as_module(code).map_err(|e| e.to_string())?;
    let mut stmts = Vec::with_capacity(module.body.len());
    for item in module.body {
        match item {
            swc_core::ecma::ast::ModuleItem::Stmt(stmt) => stmts.push(stmt),
            swc_core::ecma::ast::ModuleItem::ModuleDecl(_) => {
                return Err("import/export not allowed in Function body".to_string());
            }
        }
    }
    Ok(stmts)
}

async fn call_object_static_async(
    caller: &mut Caller<'_, RuntimeState>,
    kind: crate::types::ObjectStaticKind,
    args: &[i64],
) -> i64 {
    use crate::types::ObjectStaticKind;
    let arg0 = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    match kind {
        ObjectStaticKind::Keys
        | ObjectStaticKind::Values
        | ObjectStaticKind::Entries
        | ObjectStaticKind::GetOwnPropertyNames => {
            // 通过 re-export 的 extract 或简易实现：返回空数组作为安全回退不合适；
            match kind {
                ObjectStaticKind::Keys => {
                    crate::host_imports::object_enumerable_own_keys_async(caller, arg0).await
                }
                ObjectStaticKind::Values => {
                    crate::host_imports::object_values_async(caller, arg0).await
                }
                ObjectStaticKind::Entries => {
                    crate::host_imports::object_entries_async(caller, arg0).await
                }
                ObjectStaticKind::GetOwnPropertyNames => {
                    crate::host_imports::object_get_own_property_names_async(caller, arg0).await
                }
                _ => value::encode_undefined(),
            }
        }
        ObjectStaticKind::Assign => {
            if args.is_empty() {
                return make_type_error_exception(
                    caller,
                    "TypeError: Object.assign requires at least 1 argument",
                );
            }
            args[0]
        }
        ObjectStaticKind::Create => {
            let proto = arg0;
            if !value::is_js_object(proto) && !value::is_null(proto) {
                return make_type_error_exception(
                    caller,
                    "TypeError: Object.create prototype may only be an object or null",
                );
            }
            let Some(env) = WasmEnv::from_caller(caller) else {
                return value::encode_undefined();
            };
            if value::is_null(proto) {
                crate::runtime_heap::alloc_host_null_proto_object(caller, &env, 0)
            } else {
                let o = crate::runtime_heap::alloc_host_object(caller, &env, 0);
                crate::runtime_heap::set_object_proto_header(caller, &env, o, proto);
                o
            }
        }
        ObjectStaticKind::GetPrototypeOf => {
            // 无公开 API 时返回 null（调用方更常走 Builtin）
            value::encode_null()
        }
        ObjectStaticKind::SetPrototypeOf => {
            let proto = args.get(1).copied().unwrap_or_else(value::encode_null);
            let Some(env) = WasmEnv::from_caller(caller) else {
                return arg0;
            };
            crate::runtime_heap::set_object_proto_header(caller, &env, arg0, proto);
            arg0
        }
        ObjectStaticKind::Is => {
            let b = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            value::encode_bool(arg0 == b)
        }
        ObjectStaticKind::HasOwn => {
            let key = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            if !value::is_object(arg0) && !value::is_array(arg0) {
                return value::encode_bool(false);
            }
            let key_str = if value::is_string(key) {
                get_string_utf8_lossy(caller, key)
            } else {
                render_value(caller, key).unwrap_or_default()
            };
            let Some(ptr) = resolve_handle(caller, arg0) else {
                return value::encode_bool(false);
            };
            value::encode_bool(read_object_property_by_name(caller, ptr, &key_str).is_some())
        }
        ObjectStaticKind::FromEntries => {
            let Some(env) = WasmEnv::from_caller(caller) else {
                return value::encode_undefined();
            };
            crate::runtime_heap::alloc_host_object(caller, &env, 0)
        }
    }
}

async fn call_promise_static_async(
    caller: &mut Caller<'_, RuntimeState>,
    kind: crate::types::PromiseStaticKind,
    args: &[i64],
) -> i64 {
    use crate::types::PromiseStaticKind;
    let arg0 = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    match kind {
        PromiseStaticKind::Resolve => {
            if crate::runtime_promises::is_promise_value(caller.data(), arg0) {
                return arg0;
            }
            let entry = crate::types::PromiseEntry::pending();
            let promise = crate::runtime_promises::alloc_promise_from_caller(caller, entry);
            crate::runtime_promises::resolve_promise_from_caller(caller, promise, arg0);
            promise
        }
        PromiseStaticKind::Reject => {
            let entry = crate::types::PromiseEntry::pending();
            let promise = crate::runtime_promises::alloc_promise_from_caller(caller, entry);
            crate::runtime_promises::settle_promise(
                caller.data(),
                promise,
                PromiseSettlement::Reject(arg0),
            );
            promise
        }
        PromiseStaticKind::All
        | PromiseStaticKind::Race
        | PromiseStaticKind::AllSettled
        | PromiseStaticKind::Any => {
            let entry = crate::types::PromiseEntry::pending();
            let promise = crate::runtime_promises::alloc_promise_from_caller(caller, entry);
            crate::runtime_promises::resolve_promise_from_caller(
                caller,
                promise,
                value::encode_undefined(),
            );
            promise
        }
        PromiseStaticKind::WithResolvers => {
            let Some(env) = WasmEnv::from_caller(caller) else {
                return value::encode_undefined();
            };
            let obj = crate::runtime_heap::alloc_host_object(caller, &env, 0);
            let entry = crate::types::PromiseEntry::pending();
            let promise = crate::runtime_promises::alloc_promise_from_caller(caller, entry);
            let _ = define_host_data_property_from_caller(caller, obj, "promise", promise);
            obj
        }
    }
}
