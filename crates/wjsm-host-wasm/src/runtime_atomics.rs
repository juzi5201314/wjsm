//! Atomics 后端原语：锁内字节操作与 waiter 调度。
//!
//! ECMAScript 参数校验、TypedArray 类型分派和结果对象构造位于
//! `wjsm-builtins::atomics`；本模块只接触 Wasmtime runtime state。

use std::sync::atomic::Ordering;
use std::time::Duration;

use wasmtime::Caller;
use wjsm_host::{AtomicsRmwOp, TypedArrayView, Value};
use wjsm_ir::value;

use crate::{PromiseEntry, PromiseSettlement, RuntimeState};

fn with_buffer_mut<T>(
    caller: &mut Caller<'_, RuntimeState>,
    view: &TypedArrayView,
    byte_offset: u64,
    operation: impl FnOnce(&mut [u8]) -> T,
) -> Option<T> {
    let offset = usize::try_from(byte_offset).ok()?;
    let width = view.element_size as usize;
    if view.is_shared {
        let shared = caller.data().shared_state.as_ref()?.clone();
        let table = shared.sab_table.lock().ok()?;
        let entry = table.get(view.buffer_handle as usize)?;
        let mut data = entry.data.write().ok()?;
        let bytes = data.get_mut(offset..offset.checked_add(width)?)?;
        Some(operation(bytes))
    } else {
        let mut table = caller.data().arraybuffer_table.lock().ok()?;
        let entry = table.get_mut(view.buffer_handle as usize)?;
        let bytes = entry.data.get_mut(offset..offset.checked_add(width)?)?;
        Some(operation(bytes))
    }
}

#[inline]
fn width_mask(width: usize) -> u64 {
    if width == 8 {
        u64::MAX
    } else {
        (1_u64 << (width * 8)) - 1
    }
}

#[inline]
fn decode_raw(bytes: &[u8]) -> i64 {
    let mut raw = [0_u8; 8];
    raw[..bytes.len()].copy_from_slice(bytes);
    u64::from_le_bytes(raw) as i64
}

#[inline]
fn encode_raw(bytes: &mut [u8], raw: i64) {
    bytes.copy_from_slice(&(raw as u64).to_le_bytes()[..bytes.len()]);
}

pub(crate) fn atomic_load(
    caller: &mut Caller<'_, RuntimeState>,
    view: &TypedArrayView,
    byte_offset: u64,
) -> Option<i64> {
    with_buffer_mut(caller, view, byte_offset, |bytes| decode_raw(bytes))
}

pub(crate) fn atomic_store(
    caller: &mut Caller<'_, RuntimeState>,
    view: &TypedArrayView,
    byte_offset: u64,
    raw: i64,
) -> Option<()> {
    with_buffer_mut(caller, view, byte_offset, |bytes| encode_raw(bytes, raw))
}

pub(crate) fn atomic_rmw(
    caller: &mut Caller<'_, RuntimeState>,
    view: &TypedArrayView,
    byte_offset: u64,
    op: AtomicsRmwOp,
    operand: i64,
) -> Option<i64> {
    with_buffer_mut(caller, view, byte_offset, |bytes| {
        let mask = width_mask(bytes.len());
        let old = decode_raw(bytes) as u64 & mask;
        let operand = operand as u64 & mask;
        let next = match op {
            AtomicsRmwOp::Add => old.wrapping_add(operand),
            AtomicsRmwOp::Sub => old.wrapping_sub(operand),
            AtomicsRmwOp::And => old & operand,
            AtomicsRmwOp::Or => old | operand,
            AtomicsRmwOp::Xor => old ^ operand,
            AtomicsRmwOp::Exchange => operand,
        } & mask;
        encode_raw(bytes, next as i64);
        old as i64
    })
}

pub(crate) fn atomic_compare_exchange(
    caller: &mut Caller<'_, RuntimeState>,
    view: &TypedArrayView,
    byte_offset: u64,
    expected: i64,
    replacement: i64,
) -> Option<i64> {
    with_buffer_mut(caller, view, byte_offset, |bytes| {
        let mask = width_mask(bytes.len());
        let old = decode_raw(bytes) as u64 & mask;
        if old == expected as u64 & mask {
            encode_raw(bytes, (replacement as u64 & mask) as i64);
        }
        old as i64
    })
}

fn status(caller: &Caller<'_, RuntimeState>, value: &str) -> Value {
    crate::runtime_render::store_runtime_string(caller, value.to_string())
}

fn raw_equal(current: i64, expected: i64, width: u8) -> bool {
    let mask = width_mask(width as usize);
    current as u64 & mask == expected as u64 & mask
}

pub(crate) async fn wait_sync(
    caller: &mut Caller<'_, RuntimeState>,
    view: TypedArrayView,
    byte_offset: u64,
    expected: i64,
    timeout_ms: f64,
) -> Value {
    let Some(current) = atomic_load(caller, &view, byte_offset) else {
        return value::encode_undefined();
    };
    if !raw_equal(current, expected, view.element_size) {
        return status(caller, "not-equal");
    }
    if timeout_ms <= 0.0 {
        return status(caller, "timed-out");
    }
    let Some(shared) = caller.data().shared_state.clone() else {
        return status(caller, "timed-out");
    };
    let deadline = if timeout_ms.is_infinite() {
        None
    } else {
        Some(tokio::time::Instant::now() + Duration::from_millis(timeout_ms as u64))
    };
    let waiter = crate::shared_buffer::enter_waiter(
        &shared,
        view.buffer_handle,
        byte_offset as u32,
        deadline,
        None,
    );
    let state = if let Some(deadline) = deadline {
        tokio::select! {
            _ = waiter.signal.notified() => "ok",
            _ = tokio::time::sleep_until(deadline) => {
                crate::shared_buffer::remove_waiter(
                    &shared,
                    view.buffer_handle,
                    byte_offset as u32,
                    &waiter.notified,
                );
                if waiter.notified.load(Ordering::SeqCst) { "ok" } else { "timed-out" }
            }
        }
    } else {
        waiter.signal.notified().await;
        "ok"
    };
    status(caller, state)
}

pub(crate) fn wait_async_op(
    caller: &mut Caller<'_, RuntimeState>,
    view: TypedArrayView,
    byte_offset: u64,
    expected: i64,
    timeout_ms: f64,
) -> Value {
    let Some(current) = atomic_load(caller, &view, byte_offset) else {
        return value::encode_undefined();
    };
    if !raw_equal(current, expected, view.element_size) {
        return status(caller, "not-equal");
    }
    if timeout_ms <= 0.0 {
        return status(caller, "timed-out");
    }
    let promise = crate::alloc_promise_from_caller(caller, PromiseEntry::pending());
    let Some(shared) = caller.data().shared_state.clone() else {
        return promise;
    };
    let deadline = if timeout_ms.is_infinite() {
        None
    } else {
        Some(tokio::time::Instant::now() + Duration::from_millis(timeout_ms as u64))
    };
    let waiter = crate::shared_buffer::enter_waiter(
        &shared,
        view.buffer_handle,
        byte_offset as u32,
        deadline,
        Some(promise),
    );
    if let Some(deadline) = deadline
        && let Some(tx) = caller.data().host_completion_tx.clone()
    {
        let scope = crate::scheduler::capture_completion_scope_from_caller(caller);
        let notified = waiter.notified.clone();
        tokio::spawn(async move {
            tokio::time::sleep_until(deadline).await;
            crate::shared_buffer::remove_waiter(
                &shared,
                view.buffer_handle,
                byte_offset as u32,
                &notified,
            );
            let _ = tx.send(crate::scheduler::AsyncHostCompletion::Materialize {
                promise,
                materialize: Box::new(move |store, _env| {
                    let timed_out = crate::runtime_render::store_runtime_string_in_state(
                        store.data(),
                        "timed-out".to_string(),
                    );
                    PromiseSettlement::Fulfill(timed_out)
                }),
                scope,
            });
        });
    }
    promise
}

pub(crate) fn notify(
    caller: &mut Caller<'_, RuntimeState>,
    view: &TypedArrayView,
    byte_offset: u64,
    count: Option<u32>,
) -> u32 {
    let Some(shared) = caller.data().shared_state.clone() else {
        return 0;
    };
    let (woken, promises) = crate::shared_buffer::notify_waiters_with_promises(
        &shared,
        view.buffer_handle,
        byte_offset as u32,
        count.unwrap_or(u32::MAX),
    );
    for promise in promises {
        let ok = status(caller, "ok");
        crate::settle_promise(caller.data_mut(), promise, PromiseSettlement::Fulfill(ok));
    }
    woken
}
