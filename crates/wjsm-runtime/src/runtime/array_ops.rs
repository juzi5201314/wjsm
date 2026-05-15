use wjsm_ir::value;
use wasmtime::{Caller, Extern, Val};

use crate::types::RuntimeState;

pub(crate) fn read_array_length(caller: &mut Caller<'_, RuntimeState>, ptr: usize) -> Option<u32> {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return None;
    };
    let d = mem.data(&*caller);
    if ptr + 16 > d.len() {
        return None;
    }
    Some(u32::from_le_bytes([
        d[ptr + 8],
        d[ptr + 9],
        d[ptr + 10],
        d[ptr + 11],
    ]))
}

pub(crate) fn write_array_length(caller: &mut Caller<'_, RuntimeState>, ptr: usize, len: u32) {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return;
    };
    let d = mem.data_mut(&mut *caller);
    if ptr + 16 > d.len() {
        return;
    }
    d[ptr + 8..ptr + 12].copy_from_slice(&len.to_le_bytes());
}

/// 读取数组的 capacity 字段（offset 12）
pub(crate) fn read_array_capacity(caller: &mut Caller<'_, RuntimeState>, ptr: usize) -> Option<u32> {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return None;
    };
    let d = mem.data(&*caller);
    if ptr + 16 > d.len() {
        return None;
    }
    Some(u32::from_le_bytes([
        d[ptr + 12],
        d[ptr + 13],
        d[ptr + 14],
        d[ptr + 15],
    ]))
}

/// 读取数组元素 elements[index]（offset 16 + index * 8）
pub(crate) fn read_array_elem(caller: &mut Caller<'_, RuntimeState>, ptr: usize, index: u32) -> Option<i64> {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return None;
    };
    let d = mem.data(&*caller);
    let elem_offset = ptr + 16 + (index as usize) * 8;
    if elem_offset + 8 > d.len() {
        return None;
    }
    Some(i64::from_le_bytes([
        d[elem_offset],
        d[elem_offset + 1],
        d[elem_offset + 2],
        d[elem_offset + 3],
        d[elem_offset + 4],
        d[elem_offset + 5],
        d[elem_offset + 6],
        d[elem_offset + 7],
    ]))
}

/// 写入数组元素
pub(crate) fn write_array_elem(caller: &mut Caller<'_, RuntimeState>, ptr: usize, index: u32, val: i64) {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return;
    };
    let d = mem.data_mut(&mut *caller);
    let elem_offset = ptr + 16 + (index as usize) * 8;
    if elem_offset + 8 > d.len() {
        return;
    }
    d[elem_offset..elem_offset + 8].copy_from_slice(&val.to_le_bytes());
}

/// 数组动态扩容 — 遵循现有对象扩容的 capacity × 2 倍增策略
pub(crate) fn grow_array(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
    this_val: i64,
    new_cap: u32,
) -> Option<usize> {
    let heap_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
            return None;
        };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };
    let obj_table_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else {
            return None;
        };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };
    let new_size = 16 + new_cap as usize * 8;
    let old_size = {
        let cap = read_array_capacity(caller, ptr)?;
        16 + cap as usize * 8
    };
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return None;
    };
    let d = mem.data_mut(&mut *caller);
    if heap_ptr + new_size > d.len() {
        return None;
    }
    d.copy_within(ptr..ptr + old_size, heap_ptr);
    d[heap_ptr + 12..heap_ptr + 16].copy_from_slice(&new_cap.to_le_bytes());
    let handle_idx = (this_val as u64 & 0xFFFF_FFFF) as usize;
    let slot_addr = obj_table_ptr + handle_idx * 4;
    if slot_addr + 4 <= d.len() {
        d[slot_addr..slot_addr + 4].copy_from_slice(&(heap_ptr as u32).to_le_bytes());
    }
    if let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") {
        let _ = g.set(&mut *caller, Val::I32((heap_ptr + new_size) as i32));
    }
    Some(heap_ptr)
}
// 对象动态扩容 — 遵循 capacity × 2 倍增策略，与 grow_array 同构
// 对象槽位大小为 32 bytes（name_id:4 + flags:4 + value:8 + reserved:16）
