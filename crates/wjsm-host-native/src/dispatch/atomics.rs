//! Atomics 的 native owner。
//!
//! 所有 Atomics 方法接收 `(typed_array, index, ...)`，index 是元素索引（非字节）。
//! 只允许 shared TypedArray 上的整数类型（Int8..Uint32、BigInt64/BigUint64），
//! Float32/Float64 抛 TypeError。shared backing 用 `Arc<Mutex<Vec<u8>>>`，
//! 原子语义通过锁内 read-modify-write 实现（顺序一致性，符合 ECMAScript
//! Atomics 的 SeqCst 要求）。
//!
//! 元素以 raw bit pattern 处理：某个元素在 backing 中占 `element_size` 字节，
//! 解码为 `i64` 位模式；Number 类型按有符号/无符号解释，BigInt 类型始终按
//! 64-bit 二补数位模式解释。

use std::sync::Arc;
use std::time::Duration;

use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, range_error, to_number, type_error};
use super::typedarray::{NativeTypedArray, TypedArrayKind};
use crate::NativeAgentState;

pub(super) fn dispatch_atomics(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::AtomicsLoad => load(ctx, state, args),
        Builtin::AtomicsStore => store(ctx, state, args),
        Builtin::AtomicsAdd => rmw(ctx, state, args, RmwOp::Add),
        Builtin::AtomicsSub => rmw(ctx, state, args, RmwOp::Sub),
        Builtin::AtomicsAnd => rmw(ctx, state, args, RmwOp::And),
        Builtin::AtomicsOr => rmw(ctx, state, args, RmwOp::Or),
        Builtin::AtomicsXor => rmw(ctx, state, args, RmwOp::Xor),
        Builtin::AtomicsExchange => exchange(ctx, state, args),
        Builtin::AtomicsCompareExchange => compare_exchange(ctx, state, args),
        Builtin::AtomicsIsLockFree => is_lock_free(ctx, state, args),
        Builtin::AtomicsPause => value::encode_undefined(),
        Builtin::AtomicsWait => wait(ctx, state, args),
        Builtin::AtomicsNotify => notify(ctx, state, args),
        Builtin::AtomicsWaitAsync => wait_async(ctx, state, args),
        _ => return None,
    })
}

#[derive(Clone, Copy)]
enum RmwOp {
    Add,
    Sub,
    And,
    Or,
    Xor,
}

/// 解析 typed array 与元素索引，返回 (view, byte_offset)。
fn access(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> Result<(NativeTypedArray, usize), i64> {
    let Some(typed_array) = args.first().copied() else {
        return Err(fail_dispatch(ctx));
    };
    let Some(array) = state
        .typed_arrays
        .get(&value::decode_handle(typed_array))
        .cloned()
    else {
        return Err(type_error(
            ctx,
            state,
            "Typed array is not an integer type for Atomics",
        ));
    };
    if !array.is_shared {
        return Err(type_error(
            ctx,
            state,
            "Atomics operation must be called on a shared TypedArray",
        ));
    }
    if matches!(
        array.kind,
        TypedArrayKind::Float32 | TypedArrayKind::Float64
    ) {
        return Err(type_error(
            ctx,
            state,
            "Typed array is not an integer type for Atomics",
        ));
    }
    let Some(index) = args
        .get(1)
        .and_then(|encoded| to_number(state, *encoded))
        .and_then(|number| number.to_usize())
    else {
        return Err(fail_dispatch(ctx));
    };
    if index >= array.length {
        return Err(range_error(ctx, state, "Invalid typed array index"));
    }
    let byte_offset = array
        .offset
        .saturating_add(index)
        .saturating_mul(array.kind.element_size());
    Ok((array, byte_offset))
}

fn shared_bytes(array: &NativeTypedArray) -> Option<std::sync::Arc<std::sync::Mutex<Vec<u8>>>> {
    array.shared_buffer.clone()
}

/// 从 backing 的 `element_size` 字节读取 raw i64 位模式。
fn read_raw(bytes: &[u8], kind: TypedArrayKind) -> i64 {
    let mut buf = [0u8; 8];
    buf[..kind.element_size()].copy_from_slice(bytes);
    match kind.element_size() {
        1 => i8::from_le_bytes([buf[0]]) as i64,
        2 => i16::from_le_bytes([buf[0], buf[1]]) as i64,
        4 => i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as i64,
        8 => i64::from_le_bytes(buf),
        _ => 0,
    }
}

/// 把 raw i64 位模式写回 backing 的 `element_size` 字节。
fn write_raw(destination: &mut [u8], kind: TypedArrayKind, raw: i64) {
    let bytes = raw.to_le_bytes();
    destination.copy_from_slice(&bytes[..kind.element_size()]);
}

/// 把 JS 输入转成元素的 raw 位模式（Number 类型按有符号/无符号 wrap）。
fn js_to_raw(
    state: &mut NativeAgentState,
    kind: TypedArrayKind,
    encoded: i64,
    is_bigint: bool,
) -> Option<i64> {
    if is_bigint {
        let bigint = if let Some(bigint) = super::bigint::read(state, encoded) {
            bigint
        } else {
            let number = to_number(state, encoded)?;
            if !number.is_finite() || number.fract() != 0.0 {
                return None;
            }
            BigInt::from(number.to_i128()?)
        };
        let modulus = BigInt::from(1_u128 << 64);
        // 归一到 [0, 2^64) 的非负余数，取低 64 位即二补数位模式。
        let mut normalized = &bigint % &modulus;
        if normalized.sign() == Sign::Minus {
            normalized += &modulus;
        }
        let raw = normalized.to_u64_digits().1.first().copied().unwrap_or(0);
        return Some(raw as i64);
    }
    let number = to_number(state, encoded)?;
    Some(match kind {
        TypedArrayKind::Int8 => signed_wrap(number, 8) as i8 as i64,
        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => {
            unsigned_wrap(number, 8) as u8 as i64
        }
        TypedArrayKind::Int16 => signed_wrap(number, 16) as i16 as i64,
        TypedArrayKind::Uint16 => unsigned_wrap(number, 16) as u16 as i64,
        TypedArrayKind::Int32 => signed_wrap(number, 32) as i32 as i64,
        TypedArrayKind::Uint32 => unsigned_wrap(number, 32) as u32 as i64,
        _ => 0,
    })
}

/// 把 raw 位模式渲染为 JS boxed 值（Number 类型解码为 f64，BigInt 为 BigInt）。
fn raw_to_boxed(
    state: &mut NativeAgentState,
    kind: TypedArrayKind,
    raw: i64,
    is_bigint: bool,
) -> Option<i64> {
    if is_bigint {
        let bigint = BigInt::from(raw);
        return super::bigint::store(state, bigint);
    }
    let value = match kind {
        TypedArrayKind::Int8 => raw as i8 as f64,
        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => raw as u8 as f64,
        TypedArrayKind::Int16 => raw as i16 as f64,
        TypedArrayKind::Uint16 => raw as u16 as f64,
        TypedArrayKind::Int32 => raw as i32 as f64,
        TypedArrayKind::Uint32 => raw as u32 as f64,
        _ => 0.0,
    };
    Some(value::encode_f64(value))
}

fn signed_wrap(number: f64, bits: u32) -> f64 {
    if !number.is_finite() || number == 0.0 {
        return 0.0;
    }
    let modulus = 2_f64.powi(bits as i32);
    let mut wrapped = number.trunc().rem_euclid(modulus);
    if wrapped >= modulus / 2.0 {
        wrapped -= modulus;
    }
    wrapped
}

fn unsigned_wrap(number: f64, bits: u32) -> f64 {
    if !number.is_finite() || number == 0.0 {
        return 0.0;
    }
    number.trunc().rem_euclid(2_f64.powi(bits as i32))
}

fn load(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let (array, byte_offset) = match access(ctx, state, args) {
        Ok(access) => access,
        Err(error) => return error,
    };
    let Some(shared) = shared_bytes(&array) else {
        return fail_dispatch(ctx);
    };
    let bytes = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(raw) = bytes.get(byte_offset..byte_offset + array.kind.element_size()) else {
        return fail_dispatch(ctx);
    };
    raw_to_boxed(
        state,
        array.kind,
        read_raw(raw, array.kind),
        array.kind.is_bigint(),
    )
    .unwrap_or_else(|| fail_dispatch(ctx))
}

fn store(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let (array, byte_offset) = match access(ctx, state, args) {
        Ok(access) => access,
        Err(error) => return error,
    };
    let is_bigint = array.kind.is_bigint();
    let Some(input) = args.get(2).copied() else {
        return fail_dispatch(ctx);
    };
    let Some(raw) = js_to_raw(state, array.kind, input, is_bigint) else {
        return fail_dispatch(ctx);
    };
    let Some(shared) = shared_bytes(&array) else {
        return fail_dispatch(ctx);
    };
    let mut bytes = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let size = array.kind.element_size();
    let Some(destination) = bytes.get_mut(byte_offset..byte_offset + size) else {
        return fail_dispatch(ctx);
    };
    write_raw(destination, array.kind, raw);
    // Atomics.store 返回：Number 版本 ToIntegerOrInfinity(ToNumber(value))，
    // BigInt 版本 ToBigInt(value)。
    if is_bigint {
        raw_to_boxed(state, array.kind, raw, true).unwrap_or_else(|| fail_dispatch(ctx))
    } else if let Some(number) = to_number(state, input) {
        value::encode_f64(to_integer_or_infinity(number))
    } else {
        fail_dispatch(ctx)
    }
}

/// ToIntegerOrInfinity：NaN/±0 → +0，±Infinity 保留，其余截断。
fn to_integer_or_infinity(number: f64) -> f64 {
    if number.is_nan() || number == 0.0 {
        0.0
    } else {
        number.trunc()
    }
}

fn rmw(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64], op: RmwOp) -> i64 {
    let (array, byte_offset) = match access(ctx, state, args) {
        Ok(access) => access,
        Err(error) => return error,
    };
    let is_bigint = array.kind.is_bigint();
    let Some(input) = args.get(2).copied() else {
        return fail_dispatch(ctx);
    };
    let Some(operand) = js_to_raw(state, array.kind, input, is_bigint) else {
        return fail_dispatch(ctx);
    };
    let Some(shared) = shared_bytes(&array) else {
        return fail_dispatch(ctx);
    };
    let mut bytes = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let size = array.kind.element_size();
    let Some(slot) = bytes.get(byte_offset..byte_offset + size) else {
        return fail_dispatch(ctx);
    };
    let old = read_raw(slot, array.kind);
    let new = match op {
        RmwOp::Add => old.wrapping_add(operand),
        RmwOp::Sub => old.wrapping_sub(operand),
        RmwOp::And => old & operand,
        RmwOp::Or => old | operand,
        RmwOp::Xor => old ^ operand,
    };
    let Some(destination) = bytes.get_mut(byte_offset..byte_offset + size) else {
        return fail_dispatch(ctx);
    };
    write_raw(destination, array.kind, new);
    raw_to_boxed(state, array.kind, old, is_bigint).unwrap_or_else(|| fail_dispatch(ctx))
}

fn exchange(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let (array, byte_offset) = match access(ctx, state, args) {
        Ok(access) => access,
        Err(error) => return error,
    };
    let is_bigint = array.kind.is_bigint();
    let Some(input) = args.get(2).copied() else {
        return fail_dispatch(ctx);
    };
    let Some(converted) = js_to_raw(state, array.kind, input, is_bigint) else {
        return fail_dispatch(ctx);
    };
    let Some(shared) = shared_bytes(&array) else {
        return fail_dispatch(ctx);
    };
    let mut bytes = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let size = array.kind.element_size();
    let Some(slot) = bytes.get(byte_offset..byte_offset + size) else {
        return fail_dispatch(ctx);
    };
    let old = read_raw(slot, array.kind);
    let Some(destination) = bytes.get_mut(byte_offset..byte_offset + size) else {
        return fail_dispatch(ctx);
    };
    write_raw(destination, array.kind, converted);
    raw_to_boxed(state, array.kind, old, is_bigint).unwrap_or_else(|| fail_dispatch(ctx))
}

fn compare_exchange(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let (array, byte_offset) = match access(ctx, state, args) {
        Ok(access) => access,
        Err(error) => return error,
    };
    let is_bigint = array.kind.is_bigint();
    let Some(expected) = args.get(2).copied() else {
        return fail_dispatch(ctx);
    };
    let Some(replacement) = args.get(3).copied() else {
        return fail_dispatch(ctx);
    };
    let Some(expected) = js_to_raw(state, array.kind, expected, is_bigint) else {
        return fail_dispatch(ctx);
    };
    let Some(replacement) = js_to_raw(state, array.kind, replacement, is_bigint) else {
        return fail_dispatch(ctx);
    };
    let Some(shared) = shared_bytes(&array) else {
        return fail_dispatch(ctx);
    };
    let mut bytes = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let size = array.kind.element_size();
    let Some(slot) = bytes.get(byte_offset..byte_offset + size) else {
        return fail_dispatch(ctx);
    };
    let old = read_raw(slot, array.kind);
    if old == expected {
        let Some(destination) = bytes.get_mut(byte_offset..byte_offset + size) else {
            return fail_dispatch(ctx);
        };
        write_raw(destination, array.kind, replacement);
    }
    raw_to_boxed(state, array.kind, old, is_bigint).unwrap_or_else(|| fail_dispatch(ctx))
}

fn is_lock_free(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(size) = args
        .first()
        .and_then(|encoded| to_number(state, *encoded))
        .and_then(|number| number.to_u8())
    else {
        return fail_dispatch(ctx);
    };
    value::encode_bool(matches!(size, 1 | 2 | 4) || (size == 8 && cfg!(target_has_atomic = "64")))
}

/// wait/notify/waitAsync 专用：要求 Int32Array/BigInt64Array 且 shared。
fn waitable_access(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> Result<(NativeTypedArray, usize, u32), i64> {
    let (array, byte_offset) = access(ctx, state, args)?;
    if !matches!(
        array.kind,
        super::typedarray::TypedArrayKind::Int32 | super::typedarray::TypedArrayKind::BigInt64
    ) {
        return Err(type_error(
            ctx,
            state,
            "wait/notify/waitAsync requires Int32Array or BigInt64Array",
        ));
    }
    let Some(backing_id) = array.shared_backing_id else {
        return Err(fail_dispatch(ctx));
    };
    Ok((array, byte_offset, backing_id))
}

fn timeout_millis(state: &mut NativeAgentState, encoded: Option<i64>) -> Option<f64> {
    let Some(encoded) = encoded else {
        return Some(f64::INFINITY);
    };
    let number = to_number(state, encoded)?;
    if number.is_nan() {
        Some(f64::INFINITY)
    } else {
        Some(number.max(0.0))
    }
}

fn encode_string(state: &mut NativeAgentState, text: &str) -> i64 {
    state
        .intern_text(text.into(), wjsm_ir::value::TAG_STRING)
        .unwrap_or_else(value::encode_undefined)
}

fn wait(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let (array, byte_offset, backing_id) = match waitable_access(ctx, state, args) {
        Ok(access) => access,
        Err(error) => return error,
    };
    let is_bigint = array.kind.is_bigint();
    let Some(expected) = args.get(2).copied() else {
        return fail_dispatch(ctx);
    };
    let Some(expected) = js_to_raw(state, array.kind, expected, is_bigint) else {
        return fail_dispatch(ctx);
    };
    let Some(timeout) = timeout_millis(state, args.get(3).copied()) else {
        return fail_dispatch(ctx);
    };

    // 先比较当前位置的当前值。
    let Some(shared) = shared_bytes(&array) else {
        return fail_dispatch(ctx);
    };
    let current = {
        let bytes = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(raw) = bytes.get(byte_offset..byte_offset + array.kind.element_size()) else {
            return fail_dispatch(ctx);
        };
        read_raw(raw, array.kind)
    };
    if current != expected {
        return encode_string(state, "not-equal");
    }

    let cluster = &state.node_worker_threads.cluster;
    let registration = cluster.wait_register(backing_id, byte_offset, None);
    let timeout = if timeout.is_infinite() {
        None
    } else {
        Some(Duration::from_millis(timeout.max(0.0) as u64))
    };
    let status = cluster.wait_block(backing_id, byte_offset, &registration, timeout);
    match status {
        super::node_worker_threads::WaiterStatus::Notified => encode_string(state, "ok"),
        super::node_worker_threads::WaiterStatus::TimedOut => encode_string(state, "timed-out"),
        super::node_worker_threads::WaiterStatus::Waiting => encode_string(state, "timed-out"),
    }
}

fn notify(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let (array, byte_offset, backing_id) = match waitable_access(ctx, state, args) {
        Ok(access) => access,
        Err(error) => return error,
    };
    let count = if value::is_undefined(args.get(2).copied().unwrap_or_else(value::encode_undefined))
    {
        None
    } else {
        let Some(count) = args.get(2).and_then(|encoded| to_number(state, *encoded)) else {
            return fail_dispatch(ctx);
        };
        Some(if count.is_nan() || count <= 0.0 {
            0
        } else if count.is_infinite() {
            u32::MAX
        } else {
            count.trunc().min(u32::MAX as f64) as u32
        })
    };
    let _ = array;
    let notified = state
        .node_worker_threads
        .cluster
        .notify_waiters(backing_id, byte_offset, count);
    let woken = notified.len() as u32;
    let ok = encode_string(state, "ok");
    for promise in notified {
        super::promise::settle_promise(state, promise, ok, false);
    }
    value::encode_f64(f64::from(woken))
}

fn wait_async(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let (array, byte_offset, backing_id) = match waitable_access(ctx, state, args) {
        Ok(access) => access,
        Err(error) => return error,
    };
    let is_bigint = array.kind.is_bigint();
    let Some(expected) = args.get(2).copied() else {
        return fail_dispatch(ctx);
    };
    let Some(expected) = js_to_raw(state, array.kind, expected, is_bigint) else {
        return fail_dispatch(ctx);
    };
    let Some(timeout) = timeout_millis(state, args.get(3).copied()) else {
        return fail_dispatch(ctx);
    };

    // 比较当前值。
    let Some(shared) = shared_bytes(&array) else {
        return fail_dispatch(ctx);
    };
    let current = {
        let bytes = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(raw) = bytes.get(byte_offset..byte_offset + array.kind.element_size()) else {
            return fail_dispatch(ctx);
        };
        read_raw(raw, array.kind)
    };

    // 构造返回对象 { async, value }。
    let Ok(object) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
    let async_key = state
        .intern_text("async".into(), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx));
    let value_key = state
        .intern_text("value".into(), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx));

    if current != expected {
        // 立即 "not-equal"。
        let result = encode_string(state, "not-equal");
        if state
            .heap
            .set_property(
                value::decode_handle(object),
                value::decode_handle(async_key),
                value::encode_bool(false) as u64,
            )
            .is_err()
            || state
                .heap
                .set_property(
                    value::decode_handle(object),
                    value::decode_handle(value_key),
                    result as u64,
                )
                .is_err()
        {
            return fail_dispatch(ctx);
        }
        return object;
    }

    // current == expected：若 timeout == 0 立即 "timed-out"，否则注册 waiter + Promise。
    if timeout == 0.0 {
        let result = encode_string(state, "timed-out");
        if state
            .heap
            .set_property(
                value::decode_handle(object),
                value::decode_handle(async_key),
                value::encode_bool(false) as u64,
            )
            .is_err()
            || state
                .heap
                .set_property(
                    value::decode_handle(object),
                    value::decode_handle(value_key),
                    result as u64,
                )
                .is_err()
        {
            return fail_dispatch(ctx);
        }
        return object;
    }

    // t > 0：注册 waiter，返回已 resolve 的 Promise。
    let Some(promise) = super::promise::new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    let promise_handle = value::decode_handle(promise);
    let cluster = &state.node_worker_threads.cluster;
    let registration = cluster.wait_register(backing_id, byte_offset, Some(promise_handle));

    // 后台线程：到期后标记 TimedOut 并向 owner loop 投递。
    let timeout_cluster = Arc::clone(cluster);
    let timeout_registration = Arc::clone(&registration);
    let timeout_millis = timeout.max(0.0) as u64;
    std::thread::Builder::new()
        .name("wjsm-waitasync-timeout".into())
        .spawn(move || {
            if timeout_millis > 0 {
                std::thread::sleep(Duration::from_millis(timeout_millis));
            }
            let (lock, condvar) = &*timeout_registration;
            let mut status = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if *status == super::node_worker_threads::WaiterStatus::Waiting {
                *status = super::node_worker_threads::WaiterStatus::TimedOut;
                drop(status);
                condvar.notify_all();
                timeout_cluster.push_wait_timeout(backing_id, byte_offset, promise_handle);
            }
            timeout_cluster.remove_waiter(backing_id, byte_offset, &timeout_registration);
        })
        .ok();

    if state
        .heap
        .set_property(
            value::decode_handle(object),
            value::decode_handle(async_key),
            value::encode_bool(true) as u64,
        )
        .is_err()
        || state
            .heap
            .set_property(
                value::decode_handle(object),
                value::decode_handle(value_key),
                promise as u64,
            )
            .is_err()
    {
        return fail_dispatch(ctx);
    }
    object
}
