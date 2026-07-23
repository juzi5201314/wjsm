//! Host 侧 JS 堆读写统一入口（V2-only）。
//!
//! 全部转发到 `HeapAccessV2`；V1 memory32 `obj_table` / dyn `GcAlgorithm` 路径已删除。

use wasmtime::AsContextMut;

use crate::RuntimeState;
use crate::wasm_env::WasmEnv;
use wjsm_ir::constants;

use super::api::Handle;
use super::api::Value;

/// 解引用后的 JS 堆地址（memory64 byte offset，截断到 usize 供兼容调用方）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeapPtr {
    pub ptr: usize,
}

/// 属性槽中承载 JS value 的三种 8B 子槽。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotPart {
    Value,
    Getter,
    Setter,
}

impl SlotPart {
    #[allow(dead_code)]
    fn offset(self) -> usize {
        match self {
            Self::Value => constants::PROP_SLOT_VALUE_OFFSET as usize,
            Self::Getter => constants::PROP_SLOT_GETTER_OFFSET as usize,
            Self::Setter => constants::PROP_SLOT_SETTER_OFFSET as usize,
        }
    }
}

/// 解 handle → 对象地址（V2 handle table）。
pub fn resolve<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    _env: &WasmEnv,
    h: Handle,
) -> Option<HeapPtr> {
    let access = ctx.as_context().data().heap_access_v2().clone();
    let addr = access.resolve_handle(h).ok()?;
    Some(HeapPtr {
        ptr: addr as usize,
    })
}

/// 写属性槽 value/getter/setter。
///
/// V2 以 key 为索引；`slot_idx` 路径仅在调用方已定位到 own 槽位时使用
/// `own_property_slots` 反查 key 后写入。无法映射时返回 None。
pub fn write_property_slot<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    _env: &WasmEnv,
    h: Handle,
    slot_idx: usize,
    part: SlotPart,
    val: Value,
) -> Option<()> {
    let access = ctx.as_context().data().heap_access_v2().clone();
    let slots = access.own_property_slots(h).ok()?;
    let (key, flags) = *slots.get(slot_idx)?;
    match part {
        SlotPart::Value => {
            if flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
                // accessor 的 value 子槽无语义；忽略
                return Some(());
            }
            access.set_property(h, key, val as u64).ok()
        }
        SlotPart::Getter | SlotPart::Setter => {
            let slot = access.get_property_slot(h, key).ok()??;
            let getter = if matches!(part, SlotPart::Getter) {
                val as u64
            } else {
                slot.getter
            };
            let setter = if matches!(part, SlotPart::Setter) {
                val as u64
            } else {
                slot.setter
            };
            access
                .define_accessor_property_with_flags(h, key, getter, setter, flags)
                .ok()
        }
    }
}

/// 写数组元素槽。
pub fn write_element<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    _env: &WasmEnv,
    h: Handle,
    idx: usize,
    val: Value,
) -> Option<()> {
    let access = ctx.as_context().data().heap_access_v2().clone();
    let index = u32::try_from(idx).ok()?;
    access.set_element(h, index, val as u64).ok()
}

/// 旧 host API 只持短生命周期 ptr 时使用：V2 下 ptr 即 memory64 地址，反查 handle 后写元素。
pub fn write_element_at_ptr<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    ptr: usize,
    idx: usize,
    val: Value,
) -> Option<()> {
    let h = handle_for_object_addr(ctx, env, ptr as u64)?;
    write_element(ctx, env, h, idx, val)
}

/// 写 proto header。
pub fn write_proto<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    _env: &WasmEnv,
    h: Handle,
    proto: u32,
) -> Option<()> {
    let access = ctx.as_context().data().heap_access_v2().clone();
    access.set_prototype(h, proto).ok()
}

fn handle_for_object_addr<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    object: u64,
) -> Option<Handle> {
    let count = env
        .obj_table_count
        .get(&mut *ctx)
        .i32()
        .unwrap_or(0)
        .max(0) as u32;
    let access = ctx.as_context().data().heap_access_v2().clone();
    for h in 0..count {
        if access.resolve_handle(h).ok() == Some(object) {
            return Some(h);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_part_offsets_match_property_slot_layout() {
        assert_eq!(SlotPart::Value.offset(), 8);
        assert_eq!(SlotPart::Getter.offset(), 16);
        assert_eq!(SlotPart::Setter.offset(), 24);
    }
}
