//! 共享对象图 walker + 可插拔 RemapPolicy。
//!
//! Snapshot 恢复与 realm 克隆都要扫对象图，但改写语义不同：
//! - [`FuncTableIndexRangePolicy`]：仅平移属性槽内 `TAG_FUNCTION` 的 WASM 表索引
//! - [`ObjectHandleMapPolicy`]：按 handle map 重写对象/数组句柄与 proto header

use std::collections::HashMap;

#[cfg(feature = "managed-heap-v2")]
use crate::heap::HandleId;
use anyhow::Result;
use wjsm_ir::constants::{
    FLAG_IS_ACCESSOR, HEAP_ARRAY_CAPACITY_OFFSET, HEAP_ARRAY_ELEMENT_SIZE,
    HEAP_OBJECT_CAPACITY_OFFSET, HEAP_OBJECT_HEADER_SIZE, HEAP_OBJECT_PROPERTY_SLOT_SIZE,
    HEAP_OBJECT_PROTO_OFFSET, HEAP_OBJECT_TYPE_OFFSET, PROP_SLOT_FLAGS_OFFSET,
    PROP_SLOT_GETTER_OFFSET, PROP_SLOT_SETTER_OFFSET, PROP_SLOT_SIZE, PROP_SLOT_VALUE_OFFSET,
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

    #[cfg(feature = "managed-heap-v2")]
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

    /// 是否处理 accessor 的 getter/setter（FuncTable 策略保持跳过）。
    fn visit_accessors(&self) -> bool;

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

    fn visit_accessors(&self) -> bool {
        false
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

    fn visit_accessors(&self) -> bool {
        true
    }

    fn visit_array_elements(&self) -> bool {
        true
    }
}

/// 线性扫 heap 字节切片，按 policy 就地改写 OBJECT/ARRAY 槽。
pub fn walk_and_remap_heap(heap: &mut [u8], policy: &dyn RemapPolicy) -> Result<()> {
    let heap_end = heap.len();
    let mut ptr = 0usize;
    while ptr + HEAP_OBJECT_HEADER_SIZE as usize <= heap_end {
        let heap_type = heap[ptr + HEAP_OBJECT_TYPE_OFFSET as usize];
        let (capacity_offset, elem_size) = if heap_type == HEAP_TYPE_ARRAY {
            (HEAP_ARRAY_CAPACITY_OFFSET, HEAP_ARRAY_ELEMENT_SIZE)
        } else if heap_type == HEAP_TYPE_OBJECT {
            (HEAP_OBJECT_CAPACITY_OFFSET, HEAP_OBJECT_PROPERTY_SLOT_SIZE)
        } else {
            ptr += 1;
            continue;
        };
        let cap_start = ptr + capacity_offset as usize;
        let capacity =
            u32::from_le_bytes(heap[cap_start..cap_start + 4].try_into().expect("capacity"));
        let obj_size = (HEAP_OBJECT_HEADER_SIZE as usize)
            .saturating_add(capacity as usize * elem_size as usize);
        if obj_size == 0 || ptr.saturating_add(obj_size) > heap_end {
            break;
        }

        if heap_type == HEAP_TYPE_OBJECT {
            remap_object_at(heap, ptr, capacity, policy)?;
        } else if heap_type == HEAP_TYPE_ARRAY && policy.visit_array_elements() {
            remap_array_elements_at(heap, ptr, capacity, policy)?;
        }

        ptr += obj_size;
    }
    Ok(())
}

/// 对单个 OBJECT 地址（含 proto + 属性槽）应用 policy。
pub fn remap_object_at(
    heap: &mut [u8],
    ptr: usize,
    capacity: u32,
    policy: &dyn RemapPolicy,
) -> Result<()> {
    let heap_end = heap.len();
    // proto header
    let proto_off = ptr + HEAP_OBJECT_PROTO_OFFSET as usize;
    if proto_off + 4 <= heap_end {
        let old = u32::from_le_bytes(heap[proto_off..proto_off + 4].try_into().expect("proto"));
        let new_h = policy.remap_proto_handle(old);
        if new_h != old {
            heap[proto_off..proto_off + 4].copy_from_slice(&new_h.to_le_bytes());
        }
    }

    let props_base = ptr + HEAP_OBJECT_HEADER_SIZE as usize;
    for slot in 0..capacity as usize {
        let slot_off = props_base + slot * PROP_SLOT_SIZE as usize;
        if slot_off + PROP_SLOT_SIZE as usize > heap_end {
            break;
        }
        let flags_off = slot_off + PROP_SLOT_FLAGS_OFFSET as usize;
        let flags = i32::from_le_bytes(heap[flags_off..flags_off + 4].try_into().expect("flags"));
        if flags & FLAG_IS_ACCESSOR != 0 {
            if !policy.visit_accessors() {
                continue;
            }
            rewrite_i64_slot(heap, slot_off + PROP_SLOT_GETTER_OFFSET as usize, policy);
            rewrite_i64_slot(heap, slot_off + PROP_SLOT_SETTER_OFFSET as usize, policy);
        } else {
            rewrite_i64_slot(heap, slot_off + PROP_SLOT_VALUE_OFFSET as usize, policy);
        }
    }
    Ok(())
}

fn remap_array_elements_at(
    heap: &mut [u8],
    ptr: usize,
    capacity: u32,
    policy: &dyn RemapPolicy,
) -> Result<()> {
    let heap_end = heap.len();
    let elems_base = ptr + HEAP_OBJECT_HEADER_SIZE as usize;
    for i in 0..capacity as usize {
        let off = elems_base + i * HEAP_ARRAY_ELEMENT_SIZE as usize;
        if off + 8 > heap_end {
            break;
        }
        rewrite_i64_slot(heap, off, policy);
    }
    Ok(())
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
