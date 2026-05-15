use wasmtime::*;
use wjsm_ir::value;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let iterator_from = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            if let Some(string_data) = read_value_string_bytes(&mut caller, val) {
                let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                let handle = iters.len() as u32;
                iters.push(IteratorState::StringIter {
                    data: string_data,
                    byte_pos: 0,
                });
                return value::encode_handle(value::TAG_ITERATOR, handle);
            }

            if value::is_array(val) {
                if let Some(ptr) = resolve_handle(&mut caller, val) {
                    let length = read_array_length(&mut caller, ptr).unwrap_or(0);
                    let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                    let handle = iters.len() as u32;
                    iters.push(IteratorState::ArrayIter {
                        ptr,
                        index: 0,
                        length,
                    });
                    return value::encode_handle(value::TAG_ITERATOR, handle);
                }
            }

            if value::is_object(val) || value::is_function(val) {
                if let Some(ptr) = resolve_handle(&mut caller, val) {
                    if let Some(next) = read_object_property_by_name(&mut caller, ptr, "next") {
                        if value::is_callable(next) {
                            let return_method =
                                read_object_property_by_name(&mut caller, ptr, "return")
                                    .filter(|candidate| value::is_callable(*candidate));
                            let mut iters =
                                caller.data().iterators.lock().expect("iterators mutex");
                            let handle = iters.len() as u32;
                            iters.push(IteratorState::ObjectIter {
                                next,
                                return_method,
                                current_value: value::encode_undefined(),
                                has_current: false,
                                done: false,
                            });
                            return value::encode_handle(value::TAG_ITERATOR, handle);
                        }
                    }
                }
            }

            let mut iters = caller.data().iterators.lock().expect("iterators mutex");
            let handle = iters.len() as u32;
            iters.push(IteratorState::Error);
            value::encode_handle(value::TAG_ITERATOR, handle)
        },
    );

    // ── Import 5: iterator_next(i64) → i64 ──────────────────────────────

    let iterator_next = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let table = caller.get_export("__table").and_then(|e| e.into_table());
            let Some(func_table) = table else {
                return value::encode_undefined();
            };
            let next = {
                let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                let Some(iter) = iters.get_mut(handle_idx) else {
                    return value::encode_undefined();
                };
                match iter {
                    IteratorState::StringIter { byte_pos, .. } => {
                        *byte_pos += 1;
                        return value::encode_undefined();
                    }
                    IteratorState::ArrayIter { index, .. } => {
                        *index += 1;
                        return value::encode_undefined();
                    }
                    IteratorState::ObjectIter { next, .. } => *next,
                    IteratorState::Error => return value::encode_undefined(),
                }
            };
            let (result, current_value, done, has_current) =
                advance_object_iterator_from_caller(&mut caller, &func_table, next);
            if let Some(IteratorState::ObjectIter {
                current_value: stored_value,
                done: stored_done,
                has_current: stored_has_current,
                ..
            }) = caller
                .data()
                .iterators
                .lock()
                .expect("iterators mutex")
                .get_mut(handle_idx)
            {
                *stored_value = current_value;
                *stored_done = done;
                *stored_has_current = has_current;
            }
            result
        },
    );

    // ── Import 6: iterator_close(i64) → () ──────────────────────────────

    let iterator_close = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, handle: i64| {
            let handle_idx = value::decode_handle(handle) as usize;
            let return_method = {
                let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                match iters.get_mut(handle_idx) {
                    Some(IteratorState::ObjectIter {
                        return_method,
                        done,
                        ..
                    }) if !*done => *return_method,
                    _ => None,
                }
            };

            let Some(return_method) = return_method else {
                return;
            };
            let table = caller.get_export("__table").and_then(|e| e.into_table());
            let Some(func_table) = table else { return };
            let result = call_host_function_from_caller(
                &mut caller,
                &func_table,
                return_method,
                value::encode_undefined(),
            );
            if let Some(result) = result {
                if !(value::is_object(result)
                    || value::is_function(result)
                    || value::is_array(result))
                {
                    set_runtime_error(
                        caller.data(),
                        "TypeError: iterator return must return an object".to_string(),
                    );
                }
            }
            if let Some(IteratorState::ObjectIter { done, .. }) = caller
                .data()
                .iterators
                .lock()
                .expect("iterators mutex")
                .get_mut(handle_idx)
            {
                *done = true;
            }
        },
    );

    let iterator_value = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let mut iters = caller.data().iterators.lock().expect("iterators mutex");
            if let Some(iter) = iters.get_mut(handle_idx) {
                match iter {
                    IteratorState::StringIter { data, byte_pos } => {
                        if *byte_pos < data.len() {
                            let ch = data[*byte_pos] as char;
                            drop(iters);
                            store_runtime_string(&caller, ch.to_string())
                        } else {
                            value::encode_undefined()
                        }
                    }
                    IteratorState::ArrayIter { ptr, index, length } => {
                        if *index < *length {
                            let idx = *index;
                            let arr_ptr = *ptr;
                            drop(iters);
                            read_array_elem(&mut caller, arr_ptr, idx)
                                .unwrap_or(value::encode_undefined())
                        } else {
                            value::encode_undefined()
                        }
                    }
                    IteratorState::ObjectIter { current_value, .. } => *current_value,
                    IteratorState::Error => {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .expect("runtime error mutex") =
                            Some("TypeError: value is not iterable".to_string());
                        value::encode_undefined()
                    }
                }
            } else {
                value::encode_undefined()
            }
        },
    );

    // ── Import 8: iterator_done(i64) → i64 ──────────────────────────────

    let iterator_done = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let next = {
                let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                let Some(iter) = iters.get_mut(handle_idx) else {
                    return value::encode_bool(true);
                };
                match iter {
                    IteratorState::StringIter { data, byte_pos } => {
                        return value::encode_bool(*byte_pos >= data.len());
                    }
                    IteratorState::ArrayIter { index, length, .. } => {
                        return value::encode_bool(*index >= *length);
                    }
                    IteratorState::ObjectIter {
                        next,
                        done,
                        has_current,
                        ..
                    } => {
                        if *done {
                            return value::encode_bool(true);
                        }
                        if *has_current {
                            return value::encode_bool(*done);
                        }
                        *next
                    }
                    IteratorState::Error => {
                        set_runtime_error(
                            caller.data(),
                            "TypeError: value is not iterable".to_string(),
                        );
                        return value::encode_bool(true);
                    }
                }
            };

            let table = caller.get_export("__table").and_then(|e| e.into_table());
            let Some(func_table) = table else {
                return value::encode_bool(true);
            };
            let (_, next_value, next_done, has_current) =
                advance_object_iterator_from_caller(&mut caller, &func_table, next);
            if let Some(IteratorState::ObjectIter {
                current_value,
                done,
                has_current: stored_has_current,
                ..
            }) = caller
                .data()
                .iterators
                .lock()
                .expect("iterators mutex")
                .get_mut(handle_idx)
            {
                *current_value = next_value;
                *done = next_done;
                *stored_has_current = has_current;
            }
            value::encode_bool(next_done)
        },
    );

    // ── Import 9: enumerator_from(i64) → i64 ────────────────────────────

    let enumerator_from = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            if let Some(string_data) = read_value_string_bytes(&mut caller, val) {
                // 字符串枚举：遍历字节索引
                let len = string_data.len();
                let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::StringEnum {
                    length: len,
                    index: 0,
                });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else if value::is_object(val) || value::is_function(val) {
                // 对象/函数属性枚举
                let keys = enumerate_object_keys(&mut caller, val);
                let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::ObjectEnum { keys, index: 0 });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else if value::is_f64(val) {
                // 数字：无枚举属性（JS 语义：for..in on number = no iteration）
                let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::StringEnum {
                    length: 0,
                    index: 0,
                });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else if value::is_bool(val) {
                // 布尔值：无枚举属性（JS 语义：for..in on boolean = no iteration）
                let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::StringEnum {
                    length: 0,
                    index: 0,
                });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else {
                let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::Error);
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            }
        },
    );

    // ── Import 10: enumerator_next(i64) → i64 ───────────────────────────

    let enumerator_next = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
            if let Some(enm) = enums.get_mut(handle_idx) {
                match enm {
                    EnumeratorState::StringEnum { length, index } => {
                        if *index < *length {
                            *index += 1;
                        }
                    }
                    EnumeratorState::ObjectEnum { keys, index } => {
                        if *index < keys.len() {
                            *index += 1;
                        }
                    }
                    EnumeratorState::Error => {}
                }
            }
            value::encode_undefined()
        },
    );

    // ── Import 11: enumerator_key(i64) → i64 ────────────────────────────

    let enumerator_key = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
            if let Some(enm) = enums.get_mut(handle_idx) {
                match enm {
                    EnumeratorState::StringEnum { index, .. } => {
                        let key = index.to_string();
                        drop(enums);
                        return store_runtime_string(&caller, key);
                    }
                    EnumeratorState::ObjectEnum { keys, index } => {
                        let key = keys.get(*index).cloned().unwrap_or_default();
                        drop(enums);
                        return store_runtime_string(&caller, key);
                    }
                    EnumeratorState::Error => {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .expect("runtime error mutex") =
                            Some("TypeError: value is not enumerable".to_string());
                        return value::encode_undefined();
                    }
                }
            }
            value::encode_undefined()
        },
    );

    // ── Import 12: enumerator_done(i64) → i64 ───────────────────────────

    let enumerator_done = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
            let done = if let Some(enm) = enums.get_mut(handle_idx) {
                match enm {
                    EnumeratorState::StringEnum { length, index } => *index >= *length,
                    EnumeratorState::ObjectEnum { keys, index } => *index >= keys.len(),
                    EnumeratorState::Error => {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .expect("runtime error mutex") =
                            Some("TypeError: value is not enumerable".to_string());
                        true
                    }
                }
            } else {
                true
            };
            value::encode_bool(done)
        },
    );

    // ── Import 13: typeof(i64) → i64 ───────────────────────────────────────

    vec![
        (4, iterator_from),
        (5, iterator_next),
        (6, iterator_close),
        (7, iterator_value),
        (8, iterator_done),
        (9, enumerator_from),
        (10, enumerator_next),
        (11, enumerator_key),
        (12, enumerator_done),
    ]
}
