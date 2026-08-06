//! 共享对象图 walker + 可插拔 RemapPolicy。
//!
//! Snapshot 恢复与 realm 克隆都要扫对象图，但改写语义不同：
//! - [`FuncTableIndexRangePolicy`]：仅平移值槽内 `TAG_FUNCTION` 的 WASM 表索引
//! - [`ObjectHandleMapPolicy`]：按 handle map 重写对象/数组句柄与 proto header
//!
//! 隐藏类重构后堆内 payload 只有一种形态：`16 + capacity * 8`，每槽一个 boxed i64。
//! 属性名与 flags 全在宿主 `ShapeTable`，所以这里既不区分数据/accessor 槽，
//! 也不需要 shape 表——对象与数组走同一套遍历，只在容量字段位置上不同。
//! 未使用的值槽恒为 0，`remap_value` 对它是恒等变换。

use std::collections::HashMap;

use crate::heap::HandleId;
use anyhow::Result;
use wjsm_ir::constants::{
    HEAP_ARRAY_CAPACITY_OFFSET, HEAP_OBJECT_HEADER_SIZE, HEAP_OBJECT_PROTO_OFFSET,
    HEAP_OBJECT_TYPE_OFFSET, HEAP_OBJECT_VALUE_CAPACITY_OFFSET, HEAP_OBJECT_VALUE_SLOT_SIZE,
};
use wjsm_ir::value;
use wjsm_ir::value::TAG_ARRAY;
use wjsm_ir::{HEAP_TYPE_ARRAY, HEAP_TYPE_OBJECT};

/// old_handle_index → new_handle_index（裸 u32 handle，不是完整 i64）。
#[derive(Debug, Clone, Default)]
pub struct HandleMap {
    map: HashMap<u32, u32>,
}

impl HandleMap {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn insert(&mut self, old: u32, new: u32) {
        self.map.insert(old, new);
    }

    pub fn get(&self, old: u32) -> Option<u32> {
        self.map.get(&old).copied()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// 迭代 (old, new) 对。
    pub fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.map.iter().map(|(&k, &v)| (k, v))
    }

    /// 迭代 new handle。
    pub fn values(&self) -> impl Iterator<Item = u32> + '_ {
        self.map.values().copied()
    }

    pub fn remap_handle_v2(&self, handle: HandleId) -> HandleId {
        HandleId::new(self.get(handle.get()).unwrap_or(handle.get()))
    }
}

/// 对象图槽位改写策略。
pub trait RemapPolicy {
    /// 对扫描到的 i64 槽位改写。
    fn remap_value(&self, raw: i64) -> i64;

    /// 改写 OBJECT header 中的 proto handle（裸 u32）。
    fn remap_proto_handle(&self, h: u32) -> u32;

    /// 是否处理 ARRAY 元素槽。
    fn visit_array_elements(&self) -> bool;
}

/// Snapshot 恢复：属性槽 function idx 落在 snapshot 区间内则平移到 current_base。
pub struct FuncTableIndexRangePolicy {
    pub snapshot_base: u32,
    pub table_len: u32,
    pub current_base: u32,
}

impl RemapPolicy for FuncTableIndexRangePolicy {
    fn remap_value(&self, raw: i64) -> i64 {
        if !value::is_function(raw) {
            return raw;
        }
        let table_idx = value::decode_function_idx(raw);
        let snapshot_end = self.snapshot_base.saturating_add(self.table_len);
        if table_idx < self.snapshot_base || table_idx >= snapshot_end {
            return raw;
        }
        value::encode_function_idx(self.current_base + (table_idx - self.snapshot_base))
    }

    fn remap_proto_handle(&self, h: u32) -> u32 {
        h
    }

    fn visit_array_elements(&self) -> bool {
        false
    }
}

/// Realm 克隆：按 handle map 重写堆内对象/数组句柄；函数表索引默认不改。
pub struct ObjectHandleMapPolicy<'a> {
    pub map: &'a HandleMap,
}

impl RemapPolicy for ObjectHandleMapPolicy<'_> {
    fn remap_value(&self, raw: i64) -> i64 {
        if value::is_object(raw) {
            let old = value::decode_object_handle(raw);
            if let Some(new_h) = self.map.get(old) {
                return value::encode_object_handle(new_h);
            }
            return raw;
        }
        if value::is_array(raw) {
            let old = value::decode_array_handle(raw);
            if let Some(new_h) = self.map.get(old) {
                return value::encode_handle(TAG_ARRAY, new_h);
            }
            return raw;
        }
        // function table idx / side-table 索引（closure/bound/native/…）默认不改：
        // 克隆后方法仍指向同一 WASM 表项与共享侧表，与 Node 共享内建实现一致。
        raw
    }

    fn remap_proto_handle(&self, h: u32) -> u32 {
        // u32::MAX 常作 null proto sentinel
        if h == u32::MAX {
            return h;
        }
        self.map.get(h).unwrap_or(h)
    }

    fn visit_array_elements(&self) -> bool {
        true
    }
}

/// 线性扫 heap 字节切片，按 policy 就地改写 OBJECT/ARRAY 值槽。
pub fn walk_and_remap_heap(heap: &mut [u8], policy: &dyn RemapPolicy) -> Result<()> {
    let heap_end = heap.len();
    let mut ptr = 0usize;
    while ptr + HEAP_OBJECT_HEADER_SIZE as usize <= heap_end {
        let heap_type = heap[ptr + HEAP_OBJECT_TYPE_OFFSET as usize];
        // 容量字段位置是对象与数组的唯一差别：对象在 `+8`（`+12` 是 shape_id），
        // 数组在 `+12`（`+8` 是 length）。槽尺寸两者同为 8 字节。
        let capacity_offset = if heap_type == HEAP_TYPE_ARRAY {
            HEAP_ARRAY_CAPACITY_OFFSET
        } else if heap_type == HEAP_TYPE_OBJECT {
            HEAP_OBJECT_VALUE_CAPACITY_OFFSET
        } else {
            ptr += 1;
            continue;
        };
        let cap_start = ptr + capacity_offset as usize;
        let capacity =
            u32::from_le_bytes(heap[cap_start..cap_start + 4].try_into().expect("capacity"));
        let obj_size = (HEAP_OBJECT_HEADER_SIZE as usize)
            .saturating_add(capacity as usize * HEAP_OBJECT_VALUE_SLOT_SIZE as usize);
        if obj_size == 0 || ptr.saturating_add(obj_size) > heap_end {
            break;
        }

        if heap_type == HEAP_TYPE_OBJECT {
            remap_object_at(heap, ptr, capacity, policy)?;
        } else if policy.visit_array_elements() {
            remap_value_slots_at(heap, ptr, capacity, policy);
        }

        ptr += obj_size;
    }
    Ok(())
}

/// 对单个 OBJECT 地址（含 proto header + 全部值槽）应用 policy。
pub fn remap_object_at(
    heap: &mut [u8],
    ptr: usize,
    capacity: u32,
    policy: &dyn RemapPolicy,
) -> Result<()> {
    let proto_off = ptr + HEAP_OBJECT_PROTO_OFFSET as usize;
    if proto_off + 4 <= heap.len() {
        let old = u32::from_le_bytes(heap[proto_off..proto_off + 4].try_into().expect("proto"));
        let new_h = policy.remap_proto_handle(old);
        if new_h != old {
            heap[proto_off..proto_off + 4].copy_from_slice(&new_h.to_le_bytes());
        }
    }
    remap_value_slots_at(heap, ptr, capacity, policy);
    Ok(())
}

/// 改写 `[ptr + 16, ptr + 16 + capacity * 8)` 区间内的每个值槽。
fn remap_value_slots_at(heap: &mut [u8], ptr: usize, capacity: u32, policy: &dyn RemapPolicy) {
    let heap_end = heap.len();
    let slots_base = ptr + HEAP_OBJECT_HEADER_SIZE as usize;
    for index in 0..capacity as usize {
        let off = slots_base + index * HEAP_OBJECT_VALUE_SLOT_SIZE as usize;
        if off + 8 > heap_end {
            break;
        }
        rewrite_i64_slot(heap, off, policy);
    }
}

fn rewrite_i64_slot(heap: &mut [u8], off: usize, policy: &dyn RemapPolicy) {
    if off + 8 > heap.len() {
        return;
    }
    let raw = i64::from_le_bytes(heap[off..off + 8].try_into().expect("i64 slot"));
    let remapped = policy.remap_value(raw);
    if remapped != raw {
        heap[off..off + 8].copy_from_slice(&remapped.to_le_bytes());
    }
}
