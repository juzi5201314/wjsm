//! Async overrides for `define_core` reentrant host imports (`op_in`).

use anyhow::Result;
use wasmtime::{Caller, Linker};

use super::core::op_in_impl;
use crate::*;

pub(crate) fn define_core_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "op_in",
        |mut caller: Caller<'_, RuntimeState>, (object, prop): (i64, i64)| {
            Box::new(async move {
                if value::is_proxy(object) {
                    let handle = value::decode_proxy_handle(object) as usize;
                    let entry = {
                        let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                        table.get(handle).cloned()
                    };
                    if let Some(entry) = entry {
                        if entry.revoked {
                            return make_type_error_exception(
                                &mut caller,
                                "TypeError: Cannot perform 'has' on a proxy that has been revoked",
                            );
                        }
                        if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                            let trap =
                                read_object_property_by_name(&mut caller, handler_ptr, "has")
                                    .unwrap_or_else(value::encode_undefined);
                            if !value::is_undefined(trap) && !value::is_null(trap) {
                                let result = call_wasm_callback_async(
                                    &mut caller,
                                    trap,
                                    entry.handler,
                                    &[entry.target, prop],
                                )
                                .await
                                .unwrap_or_else(|_| value::encode_bool(false));
                                return value::encode_bool(nanbox_to_bool(result));
                            }
                        }
                        if value::is_proxy(entry.target) {
                            return Box::pin(op_in_async(&mut caller, entry.target, prop)).await;
                        }
                        return op_in_impl(&mut caller, entry.target, prop);
                    }
                    return value::encode_bool(false);
                }
                op_in_impl(&mut caller, object, prop)
            })
        },
    )?;
    async fn op_in_async(caller: &mut Caller<'_, RuntimeState>, object: i64, prop: i64) -> i64 {
        if value::is_proxy(object) {
            let handle = value::decode_proxy_handle(object) as usize;
            let entry = {
                let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                table.get(handle).cloned()
            };
            if let Some(entry) = entry {
                if entry.revoked {
                    return make_type_error_exception(
                        caller,
                        "TypeError: Cannot perform 'has' on a proxy that has been revoked",
                    );
                }
                if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                    let trap = read_object_property_by_name(caller, handler_ptr, "has")
                        .unwrap_or_else(value::encode_undefined);
                    if !value::is_undefined(trap) && !value::is_null(trap) {
                        let result = call_wasm_callback_async(
                            caller,
                            trap,
                            entry.handler,
                            &[entry.target, prop],
                        )
                        .await
                        .unwrap_or_else(|_| value::encode_bool(false));
                        return value::encode_bool(nanbox_to_bool(result));
                    }
                }
                return Box::pin(op_in_async(caller, entry.target, prop)).await;
            }
            return value::encode_bool(false);
        }
        op_in_impl(caller, object, prop)
    }

    async fn iterator_next_async(caller: &mut Caller<'_, RuntimeState>, handle: i64) -> i64 {
        let handle_idx = value::decode_handle(handle) as usize;
        let table = caller.get_export("__table").and_then(|e| e.into_table());
        let Some(func_table) = table else {
            return value::encode_undefined();
        };
        let (iterator, next) = {
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
                IteratorState::MapKeyIter { index, .. } => {
                    *index += 1;
                    return value::encode_undefined();
                }
                IteratorState::MapValueIter { index, .. } => {
                    *index += 1;
                    return value::encode_undefined();
                }
                IteratorState::TypedArrayValueIter { index, .. }
                | IteratorState::TypedArrayEntryIter { index, .. } => {
                    *index += 1;
                    return value::encode_undefined();
                }
                IteratorState::ObjectIter { iterator, next, .. } => (*iterator, *next),
                IteratorState::Error => {
                    drop(iters);
                    return alloc_iterator_result_from_caller(
                        caller,
                        value::encode_undefined(),
                        true,
                    );
                }
            }
        };
        let (result, current_value, done, has_current) =
            advance_object_iterator_from_caller_async(caller, &func_table, iterator, next).await;
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
    }

    async fn iterator_done_async(caller: &mut Caller<'_, RuntimeState>, handle: i64) -> i64 {
        let handle_idx = value::decode_handle(handle) as usize;
        let table = caller.get_export("__table").and_then(|e| e.into_table());
        let Some(func_table) = table else {
            return value::encode_bool(true);
        };
        let (iterator, next) = {
            let mut iters = caller.data().iterators.lock().expect("iterators mutex");
            let Some(iter) = iters.get_mut(handle_idx) else {
                return value::encode_bool(true);
            };
            match iter {
                IteratorState::StringIter { byte_pos, data } => {
                    return value::encode_bool(*byte_pos as usize >= data.len());
                }
                IteratorState::ArrayIter { index, length, .. } => {
                    return value::encode_bool(*index as usize >= *length as usize);
                }
                IteratorState::MapKeyIter { index, keys } => {
                    return value::encode_bool(*index as usize >= keys.len());
                }
                IteratorState::MapValueIter { index, values } => {
                    return value::encode_bool(*index as usize >= values.len());
                }
                IteratorState::TypedArrayValueIter { index, length, .. }
                | IteratorState::TypedArrayEntryIter { index, length, .. } => {
                    return value::encode_bool(*index >= *length);
                }
                IteratorState::ObjectIter {
                    iterator,
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
                    (*iterator, *next)
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
        let (_, next_value, next_done, has_current) =
            advance_object_iterator_from_caller_async(caller, &func_table, iterator, next).await;
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
    }

    async fn iterator_close_async(caller: &mut Caller<'_, RuntimeState>, handle: i64) {
        let handle_idx = value::decode_handle(handle) as usize;
        let (iterator, return_method) = {
            let mut iters = caller.data().iterators.lock().expect("iterators mutex");
            match iters.get_mut(handle_idx) {
                Some(IteratorState::ObjectIter {
                    iterator,
                    return_method,
                    done,
                    ..
                }) if !*done => (*iterator, *return_method),
                _ => return,
            }
        };

        let Some(return_method) = return_method else {
            return;
        };
        let result =
            call_iterator_method_async(caller, return_method, iterator, value::encode_undefined())
                .await;
        if !(value::is_object(result) || value::is_function(result) || value::is_array(result))
            && !value::is_undefined(result)
        {
            set_runtime_error(
                caller.data(),
                "TypeError: iterator return must return an object".to_string(),
            );
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
    }

    linker.func_wrap_async(
        "env",
        "iterator_from",
        |mut caller: Caller<'_, RuntimeState>, (val,): (i64,)| {
            Box::new(async move { super::core::iterator_from_impl_async(&mut caller, val).await })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "iterator_next",
        |mut caller: Caller<'_, RuntimeState>, (handle,): (i64,)| {
            Box::new(async move { iterator_next_async(&mut caller, handle).await })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "iterator_done",
        |mut caller: Caller<'_, RuntimeState>, (handle,): (i64,)| {
            Box::new(async move { iterator_done_async(&mut caller, handle).await })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "iterator_close",
        |mut caller: Caller<'_, RuntimeState>, (handle,): (i64,)| {
            Box::new(async move { iterator_close_async(&mut caller, handle).await })
        },
    )?;

    Ok(())
}
