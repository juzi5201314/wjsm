//! SharedArrayBuffer 的 native owner。
//!
//! 与 `ArrayBuffer`（agent 内 `Rc<RefCell<Vec<u8>>>`）不同，SAB 的 backing
//! 必须跨同 cluster 的 agent（main + workers）共享，因此使用 `Arc<Mutex<Vec<u8>>>`，
//! 本体存放在 cluster 级 `WorkerCluster::sab_table`，各 agent 只持有同一 Arc。
//!
//! 每个 agent 的 `NativeAgentState::shared_array_buffers` 把 JS handle 映射到
//! cluster backing 的引用；`NativeTypedArray` 的 shared 视图持有同一 Arc 并
//! 标记 `is_shared`，供 Atomics 使用。

use std::sync::{Arc, Mutex};

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, range_error};
use crate::NativeAgentState;

/// cluster 级共享 backing：bytes 本体 + 元数据。
/// 同一 cluster 内所有 agent 对同一 backing_id 持有同一 `Arc<Mutex<Vec<u8>>>`。
#[derive(Clone)]
pub(crate) struct SABBacking {
    pub(crate) bytes: Arc<Mutex<Vec<u8>>>,
    pub(crate) byte_length: usize,
    pub(crate) max_byte_length: Option<usize>,
}

/// agent 内 SAB side-table 条目：backing_id + 指向 cluster 本体的引用。
#[derive(Clone)]
pub(crate) struct NativeSharedArrayBuffer {
    pub(crate) backing_id: u32,
    pub(crate) backing: SABBacking,
}

impl NativeSharedArrayBuffer {
    pub(crate) fn growable(&self) -> bool {
        self.backing.max_byte_length.is_some()
    }
    pub(crate) fn byte_length(&self) -> usize {
        self.backing.byte_length
    }
    pub(crate) fn max_byte_length(&self) -> usize {
        self.backing
            .max_byte_length
            .unwrap_or(self.backing.byte_length)
    }
    pub(crate) fn grow(&mut self, new_length: usize) -> bool {
        let Some(max) = self.backing.max_byte_length else {
            return false;
        };
        if new_length < self.backing.byte_length || new_length > max {
            return false;
        }
        {
            let mut bytes = self
                .backing
                .bytes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            bytes.resize(new_length, 0);
        }
        self.backing.byte_length = new_length;
        true
    }
    pub(crate) fn slice(&self, start: usize, end: usize) -> Vec<u8> {
        let bytes = self
            .backing
            .bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        bytes[start.min(end)..end.min(bytes.len())].to_vec()
    }
}

pub(super) fn dispatch_sab(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::SharedArrayBufferConstructor => constructor(ctx, state, args),
        Builtin::SharedArrayBufferProtoByteLength => byte_length(ctx, state, args),
        Builtin::SharedArrayBufferProtoGrowable => growable(ctx, state, args),
        Builtin::SharedArrayBufferProtoMaxByteLength => max_byte_length(ctx, state, args),
        Builtin::SharedArrayBufferProtoGrow => grow(ctx, state, args),
        Builtin::SharedArrayBufferProtoSlice => slice(ctx, state, args),
        Builtin::SharedArrayBufferSpecies => args
            .first()
            .copied()
            .unwrap_or_else(value::encode_undefined),
        _ => return None,
    })
}

/// `new SharedArrayBuffer(length)` 或 `new SharedArrayBuffer(length, { maxByteLength })`；
/// ToIndex(length)（§25.2.4.1）：可执行用户转换（Symbol / BigInt 抛
/// TypeError），实参缺失 / undefined / NaN 取 0，越界按 V8 文案 RangeError。
fn constructor(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let length = match super::buffers::to_index(ctx, state, args.first().copied()) {
        Ok(length) => length,
        Err(super::buffers::ToIndexError::Thrown(exception)) => return exception,
        Err(super::buffers::ToIndexError::OutOfRange(_)) => {
            return range_error(ctx, state, "Invalid array buffer length");
        }
    };
    // §25.2.3.1 步骤 3 GetArrayBufferMaxByteLengthOption：length >
    // maxByteLength 或越界按 V8 文案 RangeError。
    let max_byte_length =
        match super::buffers::max_byte_length_option(ctx, state, args.get(1).copied(), length) {
            Ok(max) => max,
            Err(exception) => return exception,
        };
    // §25.2.3.1 AllocateSharedArrayBuffer / CreateSharedByteDataBlock：分配
    // 失败按 V8 文案 RangeError（§6.2.9.3 步骤 2），不允许宿主 OOM abort；
    // growable SAB 按 maxByteLength 一次性预留容量，后续 grow 不再分配。
    let Some(backing_id) = state.allocate_sab_backing(length, max_byte_length) else {
        return range_error(ctx, state, "Array buffer allocation failed");
    };
    let Some(object) = allocate_shared_array_buffer(ctx, state, backing_id) else {
        return fail_dispatch(ctx);
    };
    object
}

/// 在 agent 本地物化指向既有 cluster backing 的 SAB 对象（结构化克隆 /
/// test262 agent 消息传递）：与构造器同样先物化原型再分配、创建即接线
/// [[Prototype]]。backing 不存在时 side table 不落条目，由调用方检查。
pub(crate) fn materialize_from_backing(
    state: &mut NativeAgentState,
    backing_id: u32,
) -> Option<i64> {
    let prototype = state.ensure_shared_array_buffer_prototype()?;
    let object = state.allocate_object(1, false).ok()?;
    state
        .gc
        .heap()
        .set_prototype(
            value::decode_handle(object),
            value::decode_handle(prototype),
        )
        .ok()?;
    state.insert_shared_array_buffer(value::decode_handle(object), backing_id);
    Some(object)
}

/// 为既有 cluster backing 分配挂好 [[Prototype]] 的 SharedArrayBuffer 实例：
/// 先物化 %SharedArrayBuffer.prototype% 再分配实例（物化期间的分配不会
/// 悬空尚未入根的实例对象），创建即接线原型（§25.2.3.1
/// AllocateSharedArrayBuffer）。
fn allocate_shared_array_buffer(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    backing_id: u32,
) -> Option<i64> {
    let prototype = state.ensure_shared_array_buffer_prototype()?;
    let object = state.allocate_object_with_gc_retry(ctx, 1, false).ok()?;
    state
        .gc
        .heap()
        .set_prototype(
            value::decode_handle(object),
            value::decode_handle(prototype),
        )
        .ok()?;
    state.insert_shared_array_buffer(value::decode_handle(object), backing_id);
    Some(object)
}

fn byte_length(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(length) = args
        .first()
        .and_then(|object| {
            state
                .shared_array_buffers
                .get(&value::decode_handle(*object))
        })
        .and_then(|entry| u32::try_from(entry.byte_length()).ok())
    else {
        return super::buffers::incompatible_receiver(
            ctx,
            state,
            "get SharedArrayBuffer.prototype.byteLength",
            args,
        );
    };
    value::encode_f64(f64::from(length))
}

fn growable(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(growable) = args
        .first()
        .and_then(|object| {
            state
                .shared_array_buffers
                .get(&value::decode_handle(*object))
        })
        .map(|entry| entry.growable())
    else {
        return super::buffers::incompatible_receiver(
            ctx,
            state,
            "get SharedArrayBuffer.prototype.growable",
            args,
        );
    };
    value::encode_bool(growable)
}

fn max_byte_length(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(length) = args
        .first()
        .and_then(|object| {
            state
                .shared_array_buffers
                .get(&value::decode_handle(*object))
        })
        .and_then(|entry| u32::try_from(entry.max_byte_length()).ok())
    else {
        return super::buffers::incompatible_receiver(
            ctx,
            state,
            "get SharedArrayBuffer.prototype.maxByteLength",
            args,
        );
    };
    value::encode_f64(f64::from(length))
}

fn grow(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let receiver = args.first().copied().unwrap_or_else(value::encode_undefined);
    let handle = value::decode_handle(receiver);
    // §25.2.6.4 步骤 2：品牌检查要求可增长 SAB——固定长度 SAB 与非 SAB
    // 同按 V8 incompatible receiver TypeError，且先于 ToIndex(newLength)。
    if !state
        .shared_array_buffers
        .get(&handle)
        .is_some_and(NativeSharedArrayBuffer::growable)
    {
        return super::buffers::incompatible_receiver(
            ctx,
            state,
            "SharedArrayBuffer.prototype.grow",
            args,
        );
    }
    // §25.2.6.4 步骤 3：ToIndex(newLength)——可执行用户转换（Symbol /
    // BigInt 抛 TypeError），数值越界与 shrink / 超 max 同按 V8 文案
    // RangeError。
    let invalid_length = "SharedArrayBuffer.prototype.grow: Invalid length parameter";
    let new_length = match super::buffers::to_index(ctx, state, args.get(1).copied()) {
        Ok(new_length) => new_length,
        Err(super::buffers::ToIndexError::Thrown(exception)) => return exception,
        Err(super::buffers::ToIndexError::OutOfRange(_)) => {
            return range_error(ctx, state, invalid_length);
        }
    };
    let Some(entry) = state.shared_array_buffers.get_mut(&handle) else {
        return fail_dispatch(ctx);
    };
    if entry.grow(new_length) {
        state
            .node_worker_threads
            .cluster
            .update_sab_length(entry.backing_id, new_length);
        value::encode_undefined()
    } else {
        range_error(ctx, state, invalid_length)
    }
}

fn slice(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let receiver = args.first().copied().unwrap_or_else(value::encode_undefined);
    let handle = value::decode_handle(receiver);
    let Some(entry) = state.shared_array_buffers.get(&handle).cloned() else {
        return super::buffers::incompatible_receiver(
            ctx,
            state,
            "SharedArrayBuffer.prototype.slice",
            args,
        );
    };
    // §25.2.6.6 步骤 6–9：start / end 经 ToIntegerOrInfinity（可执行用户
    // 转换，Symbol / BigInt TypeError 原样上抛）；end 为 undefined 取 len。
    let length = entry.byte_length();
    let start = match args.get(1) {
        None => 0,
        Some(encoded) => match super::buffers::relative_index(ctx, state, *encoded, length) {
            Ok(start) => start,
            Err(exception) => return exception,
        },
    };
    let end = match args.get(2) {
        None => length,
        Some(encoded) if value::is_undefined(*encoded) => length,
        Some(encoded) => match super::buffers::relative_index(ctx, state, *encoded, length) {
            Ok(end) => end,
            Err(exception) => return exception,
        },
    };
    let bytes = entry.slice(start, end);
    // 先物化原型再分配实例（同 `allocate_shared_array_buffer`）。
    let Some(prototype) = state.ensure_shared_array_buffer_prototype() else {
        return fail_dispatch(ctx);
    };
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 1, false) else {
        return fail_dispatch(ctx);
    };
    if state
        .gc
        .heap()
        .set_prototype(
            value::decode_handle(object),
            value::decode_handle(prototype),
        )
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    let backing_id = state.allocate_sab_backing_from_bytes(bytes);
    state.insert_shared_array_buffer(value::decode_handle(object), backing_id);
    object
}

