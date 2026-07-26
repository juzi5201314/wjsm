//! Async overrides for `define_core` reentrant host imports (`op_in` / iterators).
//!
//! 算法在 `wjsm-builtins::core_async`；本文件保留 async-from-sync 表推进与薄注册。

use anyhow::Result;
use wasmtime::{Caller, Linker, Store};

use crate::*;

pub(crate) fn resolve_async_from_sync_afs_handle(
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

pub(crate) async fn materialize_async_from_sync_next(
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

pub(crate) fn define_core_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    use crate::exec_context_impl::WasmExecContext;
    linker.func_wrap_async(
        "env",
        "op_in",
        |mut caller: Caller<'_, RuntimeState>, (object, prop): (i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core_async::op_in(&mut ctx, object, prop).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "iterator_from",
        |mut caller: Caller<'_, RuntimeState>, (val,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core_async::iterator_from(&mut ctx, val).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "iterator_next",
        |mut caller: Caller<'_, RuntimeState>, (handle,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core_async::iterator_next(&mut ctx, handle).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "iterator_done",
        |mut caller: Caller<'_, RuntimeState>, (handle,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core_async::iterator_done(&mut ctx, handle).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "iterator_close",
        |mut caller: Caller<'_, RuntimeState>, (handle, completion): (i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core_async::iterator_close(&mut ctx, handle, completion).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "iterator_step_value",
        |mut caller: Caller<'_, RuntimeState>, (handle,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core_async::iterator_step_value(&mut ctx, handle).await
            })
        },
    )?;
    Ok(())
}
