//! Host 侧 JS 堆读写统一入口（V2-only）。
//!
//! 全部转发到 `HeapAccessV2`；无 main-memory obj_table 路径。

use wasmtime::AsContextMut;

use crate::RuntimeState;
use crate::wasm_env::WasmEnv;
use wjsm_ir::constants;

use super::api::Handle;
use super::api::Value;

/// 属性槽中承载 JS value 的三种 8B 子槽。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotPart {
    Value,
    Getter,
    Setter,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_part_offsets_match_property_slot_layout() {
        let offset = |part: SlotPart| match part {
            SlotPart::Value => constants::PROP_SLOT_VALUE_OFFSET as usize,
            SlotPart::Getter => constants::PROP_SLOT_GETTER_OFFSET as usize,
            SlotPart::Setter => constants::PROP_SLOT_SETTER_OFFSET as usize,
        };
        assert_eq!(offset(SlotPart::Value), 8);
        assert_eq!(offset(SlotPart::Getter), 16);
        assert_eq!(offset(SlotPart::Setter), 24);
    }
}
