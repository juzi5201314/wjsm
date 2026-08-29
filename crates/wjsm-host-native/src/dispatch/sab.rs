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

use num_traits::ToPrimitive;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, to_number, type_error};
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
/// 实参缺失或 undefined 时 ToIndex 取 0（§25.2.4.1）。
fn constructor(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let length = match args.first() {
        None => 0,
        Some(encoded) if value::is_undefined(*encoded) => 0,
        Some(encoded) => {
            let Some(length) = to_number(state, *encoded).and_then(|number| number.to_usize())
            else {
                return fail_dispatch(ctx);
            };
            length
        }
    };
    let max_byte_length = args
        .get(1)
        .and_then(|options| {
            if value::is_undefined(*options) {
                None
            } else {
                super::modules::named_property(state, *options, "maxByteLength")
            }
        })
        .and_then(|encoded| to_number(state, encoded))
        .and_then(|number| number.to_usize());
    if let Some(max) = max_byte_length
        && length > max
    {
        return fail_dispatch(ctx);
    }
    let Some(object) = allocate_shared_array_buffer(ctx, state, length, max_byte_length) else {
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

/// 分配挂好 [[Prototype]] 的 SharedArrayBuffer 实例：先物化
/// %SharedArrayBuffer.prototype% 再分配实例（物化期间的分配不会悬空尚未
/// 入根的实例对象），创建即接线原型（§25.2.3.1 AllocateSharedArrayBuffer）。
fn allocate_shared_array_buffer(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    length: usize,
    max_byte_length: Option<usize>,
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
    let backing_id = state.allocate_sab_backing(length, max_byte_length);
    state.insert_shared_array_buffer(value::decode_handle(object), backing_id);
    Some(object)
}

fn byte_length(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    args.first()
        .and_then(|object| {
            state
                .shared_array_buffers
                .get(&value::decode_handle(*object))
        })
        .and_then(|entry| u32::try_from(entry.byte_length()).ok())
        .map(|length| value::encode_f64(f64::from(length)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn growable(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    args.first()
        .and_then(|object| {
            state
                .shared_array_buffers
                .get(&value::decode_handle(*object))
        })
        .map(|entry| value::encode_bool(entry.growable()))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn max_byte_length(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    args.first()
        .and_then(|object| {
            state
                .shared_array_buffers
                .get(&value::decode_handle(*object))
        })
        .and_then(|entry| u32::try_from(entry.max_byte_length()).ok())
        .map(|length| value::encode_f64(f64::from(length)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn grow(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(new_length) = args
        .get(1)
        .and_then(|encoded| to_number(state, *encoded))
        .and_then(|number| number.to_usize())
    else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(receiver);
    let Some(entry) = state.shared_array_buffers.get(&handle) else {
        return fail_dispatch(ctx);
    };
    if !entry.growable() {
        return type_error(
            ctx,
            state,
            "SharedArrayBuffer.prototype.grow can only be used with growable SharedArrayBuffers",
        );
    }
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
        fail_dispatch(ctx)
    }
}

fn slice(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(receiver);
    let Some(entry) = state.shared_array_buffers.get(&handle).cloned() else {
        return fail_dispatch(ctx);
    };
    let length = entry.byte_length();
    let start = relative_index(state, args.get(1).copied(), length);
    let end = args.get(2).map_or(length, |encoded| {
        relative_index(state, Some(*encoded), length)
    });
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

fn relative_index(state: &NativeAgentState, input: Option<i64>, length: usize) -> usize {
    let Some(encoded) = input else {
        return 0;
    };
    let Some(number) = to_number(state, encoded).and_then(|number| number.to_i64()) else {
        return 0;
    };
    if number < 0 {
        (length as i64 + number).max(0) as usize
    } else {
        (number as usize).min(length)
    }
}
