//! Async overrides for `define_core` reentrant host imports (`op_in`).

use anyhow::Result;
use wasmtime::{Caller, Linker};

use super::core::{iterator_value_impl, op_in_impl, string_iter_advance_unit_pos};
use crate::*;

pub(crate) fn define_core_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "op_in",
        |mut caller: Caller<'_, RuntimeState>, (object, prop): (i64, i64)| {
            // proxy has-trap 链与递归逻辑统一由下方 op_in_async 实现，避免内联重复。
            Box::new(async move { op_in_async(&mut caller, object, prop).await })
        },
    )?;
    async fn op_in_async(caller: &mut Caller<'_, RuntimeState>, object: i64, prop: i64) -> i64 {
        if value::is_proxy(object) {
            let handle = value::decode_proxy_handle(object) as usize;
            let entry = {
                let table = caller
                    .data()
                    .proxy_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(handle).cloned()
            };
            if let Some(entry) = entry {
                if entry.revoked {
                    return make_type_error_exception(
                        caller,
                        "TypeError: Cannot perform 'has' on a proxy that has been revoked",
                    );
                }
                #[cfg(feature = "managed-heap-v2")]
                let trap = read_host_data_property_v2(caller, entry.handler, "has")
                    .unwrap_or_else(value::encode_undefined);
                #[cfg(not(feature = "managed-heap-v2"))]
                let trap = resolve_handle(caller, entry.handler)
                    .and_then(|handler_ptr| {
                        read_object_property_by_name(caller, handler_ptr, "has")
                    })
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
                return Box::pin(op_in_async(caller, entry.target, prop)).await;
            }
            return value::encode_bool(false);
        }
        #[cfg(feature = "managed-heap-v2")]
        if value::is_js_object(object) || value::is_array(object) {
            let Some(name_id) = property_key_value_to_name_id(caller, prop, false) else {
                return value::encode_bool(false);
            };
            let Some(key) = crate::property_key::canonicalize_v2_name_id(caller, name_id) else {
                return value::encode_bool(false);
            };
            return value::encode_bool(
                caller
                    .data()
                    .heap_access_v2()
                    .get_property_slot_on_proto_chain(value::decode_handle(object), key)
                    .ok()
                    .flatten()
                    .is_some(),
            );
        }
        op_in_impl(caller, object, prop)
    }
    fn resolve_async_from_sync_afs_handle(
        caller: &Caller<'_, RuntimeState>,
        handle: i64,
        next: i64,
    ) -> Option<u32> {
        {
            let table = caller
                .data()
                .async_from_sync_iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let handle_idx = value::decode_handle(handle);
            if let Some(i) = table
                .iter()
                .position(|e| e.outer_handle_idx == handle_idx || e.outer_iter == handle)
            {
                return Some(i as u32);
            }
        }
        if value::is_native_callable(next) {
            let idx = value::decode_native_callable_idx(next);
            let nc = caller
                .data()
                .native_callables
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(NativeCallable::AsyncFromSyncNext { handle: h }) = nc.get(idx as usize) {
                return Some(*h);
            }
        }
        None
    }

    fn parse_iterator_result_fields(
        caller: &mut Caller<'_, RuntimeState>,
        result: i64,
    ) -> Option<(i64, bool)> {
        if !(value::is_object(result) || value::is_function(result) || value::is_array(result)) {
            return None;
        }
        let ptr = resolve_handle(caller, result)?;
        let done = read_object_property_by_name(caller, ptr, "done")
            .map(nanbox_to_bool)
            .unwrap_or(false);
        let current_value = read_object_property_by_name(caller, ptr, "value")
            .unwrap_or_else(value::encode_undefined);
        Some((current_value, done))
    }

    async fn materialize_async_from_sync_next(
        caller: &mut Caller<'_, RuntimeState>,
        afs_handle: u32,
    ) -> i64 {
        let outer_handle_idx = {
            let table = caller
                .data()
                .async_from_sync_iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table
                .get(afs_handle as usize)
                .map(|e| e.outer_handle_idx as usize)
                .unwrap_or(afs_handle as usize)
        };
        let promise = advance_async_from_sync_async(caller, afs_handle).await;

        if value::is_exception(promise) {
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let reason = exception_reason(caller, promise);
            settle_promise(caller.data(), p, PromiseSettlement::Reject(reason));
            return p;
        }

        if !is_promise_value(caller.data(), promise) {
            if let Some((current_value, done)) = parse_iterator_result_fields(caller, promise) {
                if let Some(IteratorState::ObjectIter {
                    current_value: stored_value,
                    done: stored_done,
                    has_current: stored_has_current,
                    ..
                }) = caller
                    .data()
                    .iterators
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_mut(outer_handle_idx)
                {
                    *stored_value = current_value;
                    *stored_done = done;
                    *stored_has_current = true;
                }
                return promise;
            }
            return promise;
        }

        let promise_handle = raw_promise_handle(promise);
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
            // advance 返回的是 rejected promise，直接返回它（不创建新 promise）
            // 避免原 promise 无 handler 产生 UnhandledPromiseRejectionWarning
            return promise;
        }
        if let Some(settled_val) = fulfilled {
            if let Some((current_value, done)) = parse_iterator_result_fields(caller, settled_val)
                && let Some(IteratorState::ObjectIter {
                    current_value: stored_value,
                    done: stored_done,
                    has_current: stored_has_current,
                    ..
                }) = caller
                    .data()
                    .iterators
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_mut(outer_handle_idx)
            {
                *stored_value = current_value;
                *stored_done = done;
                *stored_has_current = true;
            }
            return settled_val;
        }
        promise
    }

    async fn iterator_next_async(caller: &mut Caller<'_, RuntimeState>, handle: i64) -> i64 {
        // for-await 的迭代器句柄可能是 GetIterator(obj, async) 同步抛出的 TAG_EXCEPTION
        // （@@asyncIterator / @@iterator 非可调用、async-from-sync 构造失败等）。将其转为
        // rejected promise：await 该 promise 时走 is_rejected → emit_throw_value，可被
        // for-await 外层 try/catch 捕获，否则 reject 当前 async 函数 promise。这复用了
        // 循环既有的 suspend/resume 拒绝路径（与下方 A3 同步抛出处理一致），避免在异步函数
        // 体内插入 IsException 控制流分叉——那会产生跨状态机段的 catch 入边，被 relooper
        // 误编译。同步 for-of 永不会把 TAG_EXCEPTION 传入此处（IteratorFrom 总返回
        // TAG_ITERATOR/Error），故此保护不影响同步迭代。
        if value::is_exception(handle) {
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let reason = exception_reason(caller, handle);
            settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
            return promise;
        }
        let handle_idx = value::decode_handle(handle) as usize;
        if let Some(afs_handle) = {
            let table = caller
                .data()
                .async_from_sync_iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let decoded = value::decode_handle(handle);
            table
                .iter()
                .position(|e| e.outer_iter == handle || e.outer_handle_idx == decoded)
                .map(|i| i as u32)
        } {
            return materialize_async_from_sync_next(caller, afs_handle).await;
        }
        let (iterator, next) = {
            let mut iters = caller
                .data()
                .iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(iter) = iters.get_mut(handle_idx) else {
                return value::encode_undefined();
            };
            match iter {
                IteratorState::StringIter { string, unit_pos } => {
                    string_iter_advance_unit_pos(string, unit_pos);
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
                IteratorState::SetValueIter { index, .. }
                | IteratorState::SetEntryIter { index, .. } => {
                    *index += 1;
                    return value::encode_undefined();
                }
                IteratorState::MapEntryIter { index, .. } => {
                    *index += 1;
                    return value::encode_undefined();
                }
                IteratorState::HeadersKeyIter { index, .. }
                | IteratorState::HeadersValueIter { index, .. }
                | IteratorState::HeadersEntryIter { index, .. } => {
                    *index += 1;
                    return value::encode_undefined();
                }
                IteratorState::IndexValueIter { index, .. } => {
                    *index += 1;
                    return value::encode_undefined();
                }
                IteratorState::TypedArrayValueIter { index, .. }
                | IteratorState::TypedArrayEntryIter { index, .. } => {
                    *index += 1;
                    return value::encode_undefined();
                }
                IteratorState::RegExpStringIter { .. } => {
                    drop(iters);
                    regexp_string_iter_next(caller, handle_idx);
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
        if let Some(afs_handle) = resolve_async_from_sync_afs_handle(caller, handle, next) {
            return materialize_async_from_sync_next(caller, afs_handle).await;
        }
        let (result, current_value, done, has_current) =
            advance_object_iterator_from_caller_async(caller, iterator, next).await;

        // A3: 若 advance 回传异常（同步 throw），返回 rejected promise。
        // await 此 rejected promise 后会走 is_rejected → emit_throw_value（可捕获）。
        if value::is_exception(result) {
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let reason = exception_reason(caller, result);
            settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
            return promise;
        }

        let mut result = result;
        let mut current_value = current_value;
        let mut done = done;
        let mut has_current = has_current;

        // next() 返回 Promise：同步展开已 settled 的值为 IteratorResult。
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
            if let Some(reason) = rejected {
                let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
                return promise;
            }
            if let Some(settled_val) = fulfilled {
                if let Some((cv, d)) = parse_iterator_result_fields(caller, settled_val) {
                    result = settled_val;
                    current_value = cv;
                    done = d;
                    has_current = true;
                }
            } else {
                return result;
            }
        }

        if let Some(IteratorState::ObjectIter {
            current_value: stored_value,
            done: stored_done,
            has_current: stored_has_current,
            ..
        }) = caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(handle_idx)
        {
            *stored_value = current_value;
            *stored_done = done;
            *stored_has_current = has_current;
        }
        if has_current {
            if value::is_object(result) || value::is_function(result) || value::is_array(result) {
                return result;
            }
            return alloc_iterator_result_from_caller(caller, current_value, done);
        }
        if is_promise_value(caller.data(), result) {
            return result;
        }
        result
    }

    async fn iterator_done_async(caller: &mut Caller<'_, RuntimeState>, handle: i64) -> i64 {
        let handle_idx = value::decode_handle(handle) as usize;
        let (iterator, next) = {
            let mut iters = caller
                .data()
                .iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(iter) = iters.get_mut(handle_idx) else {
                return value::encode_bool(true);
            };
            match iter {
                IteratorState::StringIter { string, unit_pos } => {
                    return value::encode_bool(*unit_pos >= string.utf16_len());
                }
                IteratorState::ArrayIter { index, length, .. } => {
                    return value::encode_bool(*index as usize >= *length as usize);
                }
                IteratorState::MapKeyIter {
                    index, map_handle, ..
                } => {
                    let table = caller
                        .data()
                        .map_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let done = if *map_handle < table.len() as u32 {
                        *index as usize >= table[*map_handle as usize].keys.len()
                    } else {
                        true
                    };
                    drop(table);
                    return value::encode_bool(done);
                }
                IteratorState::MapValueIter {
                    index, map_handle, ..
                } => {
                    let table = caller
                        .data()
                        .map_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let done = if *map_handle < table.len() as u32 {
                        *index as usize >= table[*map_handle as usize].values.len()
                    } else {
                        true
                    };
                    drop(table);
                    return value::encode_bool(done);
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
                    let done = if *set_handle < table.len() as u32 {
                        *index as usize >= table[*set_handle as usize].values.len()
                    } else {
                        true
                    };
                    drop(table);
                    return value::encode_bool(done);
                }
                IteratorState::MapEntryIter {
                    index, map_handle, ..
                } => {
                    let table = caller
                        .data()
                        .map_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let done = if *map_handle < table.len() as u32 {
                        *index as usize >= table[*map_handle as usize].keys.len()
                    } else {
                        true
                    };
                    drop(table);
                    return value::encode_bool(done);
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
                    let done = if *headers_handle < table.len() as u32 {
                        *index as usize >= table[*headers_handle as usize].pairs.len()
                    } else {
                        true
                    };
                    drop(table);
                    return value::encode_bool(done);
                }
                IteratorState::IndexValueIter { index, values } => {
                    return value::encode_bool(*index as usize >= values.len());
                }
                IteratorState::TypedArrayValueIter { index, length, .. }
                | IteratorState::TypedArrayEntryIter { index, length, .. } => {
                    return value::encode_bool(*index >= *length);
                }
                IteratorState::RegExpStringIter { .. } => {
                    drop(iters);
                    return value::encode_bool(regexp_string_iter_ensure_current(
                        caller, handle_idx,
                    ));
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
            advance_object_iterator_from_caller_async(caller, iterator, next).await;
        if let Some(IteratorState::ObjectIter {
            current_value,
            done,
            has_current: stored_has_current,
            ..
        }) = caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(handle_idx)
        {
            *current_value = next_value;
            *done = next_done;
            *stored_has_current = has_current;
        }
        value::encode_bool(next_done)
    }

    /// ES §7.4.6 IteratorClose：接受 completion，在 `return` 抛错时以该异常替换 completion，
    /// 若 `return` 结果非 Object（含 undefined）则抛出 TypeError，否则返回原 completion。
    async fn iterator_close_async(
        caller: &mut Caller<'_, RuntimeState>,
        handle: i64,
        completion: i64,
    ) -> i64 {
        let handle_idx = value::decode_handle(handle) as usize;
        let (iterator, return_method) = {
            let mut iters = caller
                .data()
                .iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match iters.get_mut(handle_idx) {
                Some(IteratorState::ObjectIter {
                    iterator,
                    return_method,
                    done,
                    ..
                }) if !*done => (*iterator, *return_method),
                _ => return completion,
            }
        };

        let Some(return_method) = return_method else {
            if let Some(IteratorState::ObjectIter { done, .. }) = caller
                .data()
                .iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(handle_idx)
            {
                *done = true;
            }
            return completion;
        };

        let result =
            call_iterator_method_async(caller, return_method, iterator, value::encode_undefined())
                .await;

        if value::is_exception(result) {
            // ES §7.4.6 step 5: 原 completion 为 throw 时优先返回原 throw
            if value::is_exception(completion) && value::is_exception(result) {
                if let Some(IteratorState::ObjectIter { done, .. }) = caller
                    .data()
                    .iterators
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_mut(handle_idx)
                {
                    *done = true;
                }
                return completion;
            }
            if let Some(IteratorState::ObjectIter { done, .. }) = caller
                .data()
                .iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(handle_idx)
            {
                *done = true;
            }
            return result;
        }

        let is_object_like =
            value::is_object(result) || value::is_function(result) || value::is_array(result);
        if !is_object_like {
            if let Some(IteratorState::ObjectIter { done, .. }) = caller
                .data()
                .iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(handle_idx)
            {
                *done = true;
            }
            return make_type_error_exception(
                caller,
                "TypeError: iterator return must return an object",
            );
        }

        if let Some(IteratorState::ObjectIter { done, .. }) = caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(handle_idx)
        {
            *done = true;
        }
        completion
    }

    /// 合并 IteratorDone + IteratorValue + IteratorNext，供数组解构等线性 IR 使用
    async fn iterator_step_value_async(caller: &mut Caller<'_, RuntimeState>, handle: i64) -> i64 {
        let done = iterator_done_async(caller, handle).await;
        if value::decode_bool(done) {
            return value::encode_undefined();
        }
        let value = iterator_value_impl(caller, handle);
        let _ = iterator_next_async(caller, handle).await;
        value
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
        |mut caller: Caller<'_, RuntimeState>, (handle, completion): (i64, i64)| {
            Box::new(async move { iterator_close_async(&mut caller, handle, completion).await })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "iterator_step_value",
        |mut caller: Caller<'_, RuntimeState>, (handle,): (i64,)| {
            Box::new(async move { iterator_step_value_async(&mut caller, handle).await })
        },
    )?;

    Ok(())
}
