use std::sync::{Arc, Mutex};

use wasmtime::{Caller, Extern, Global, Memory, Store, Val};

use crate::types::*;
use crate::runtime::string_utils::{read_string_bytes, store_runtime_string, store_runtime_string_in_state, read_value_string_bytes};
use crate::runtime::format::format_number_js;
use crate::runtime::object_ops::{grow_object, read_object_property_by_name, find_property_slot_by_name_id};
use crate::runtime::eval::is_promise_value;
use wjsm_ir::{constants, value};

pub(crate) const SHADOW_STACK_SIZE: u32 = 65536;

pub(crate) fn find_memory_c_string_global(caller: &mut Caller<'_, RuntimeState>, name: &str) -> Option<u32> {
    let mut needle = Vec::with_capacity(name.len() + 1);
    needle.extend_from_slice(name.as_bytes());
    needle.push(0);
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return None;
    };
    memory
        .data(&*caller)
        .windows(needle.len())
        .position(|window| window == needle.as_slice())
        .map(|offset| offset as u32)
}

pub(crate) fn alloc_heap_c_string_global(caller: &mut Caller<'_, RuntimeState>, name: &str) -> Option<u32> {
    let heap_ptr = caller
        .get_export("__heap_ptr")
        .and_then(|e| e.into_global())?
        .get(&mut *caller)
        .i32()
        .unwrap_or(0) as usize;
    let bytes = name.as_bytes();
    let end = heap_ptr.checked_add(bytes.len() + 1)?;
    let aligned_end = (end + 7) & !7;
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data_mut(&mut *caller);
        if aligned_end > data.len() {
            return None;
        }
        data[heap_ptr..heap_ptr + bytes.len()].copy_from_slice(bytes);
        data[heap_ptr + bytes.len()] = 0;
        data[end..aligned_end].fill(0);
    }
    if let Some(Extern::Global(global)) = caller.get_export("__heap_ptr") {
        let _ = global.set(&mut *caller, Val::I32(aligned_end as i32));
    }
    Some(heap_ptr as u32)
}

pub(crate) fn alloc_host_object_from_caller(caller: &mut Caller<'_, RuntimeState>, capacity: u32) -> i64 {
    let heap_ptr = caller
        .get_export("__heap_ptr")
        .and_then(|e| e.into_global())
        .and_then(|g| g.get(&mut *caller).i32())
        .unwrap_or(0) as u32;
    let obj_table_count = caller
        .get_export("__obj_table_count")
        .and_then(|e| e.into_global())
        .and_then(|g| g.get(&mut *caller).i32())
        .unwrap_or(0) as u32;
    let obj_table_ptr = caller
        .get_export("__obj_table_ptr")
        .and_then(|e| e.into_global())
        .and_then(|g| g.get(&mut *caller).i32())
        .unwrap_or(0) as u32;
    let size = 16 + capacity * 32;
    let new_heap_ptr = heap_ptr.saturating_add(size);
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return value::encode_undefined();
        };
        let data = memory.data_mut(&mut *caller);
        let ptr = heap_ptr as usize;
        if new_heap_ptr as usize > data.len() {
            return value::encode_undefined();
        }
        data[ptr..ptr + 4].copy_from_slice(&0u32.to_le_bytes());
        data[ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT;
        data[ptr + 5..ptr + 8].fill(0);
        data[ptr + 8..ptr + 12].copy_from_slice(&capacity.to_le_bytes());
        data[ptr + 12..ptr + 16].copy_from_slice(&0u32.to_le_bytes());
        let slot_addr = (obj_table_ptr + obj_table_count * 4) as usize;
        if slot_addr + 4 <= data.len() {
            data[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
        }
    }
    if let Some(Extern::Global(global)) = caller.get_export("__heap_ptr") {
        let _ = global.set(&mut *caller, Val::I32(new_heap_ptr as i32));
    }
    if let Some(Extern::Global(global)) = caller.get_export("__obj_table_count") {
        let _ = global.set(&mut *caller, Val::I32((obj_table_count + 1) as i32));
    }
    value::encode_object_handle(obj_table_count)
}

pub(crate) fn create_error_object(caller: &mut Caller<'_, RuntimeState>, error_name: &str, arg: i64) -> i64 {
    let message = if value::is_undefined(arg) {
        String::new()
    } else if value::is_string(arg) {
        read_value_string_bytes(caller, arg)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    } else if value::is_null(arg) {
        String::new()
    } else if value::is_f64(arg) {
        format_number_js(value::decode_f64(arg))
    } else if value::is_bool(arg) {
        if value::decode_bool(arg) { "true".to_string() } else { "false".to_string() }
    } else {
        String::new()
    };
    {
        let mut table = caller.data().error_table.lock().expect("error table mutex");
        table.push(ErrorEntry {
            name: error_name.to_string(),
            message: message.clone(),
        });
    }
    let obj = alloc_host_object_from_caller(caller, 2);
    let name_val = store_runtime_string(caller, error_name.to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "name", name_val);
    let message_val = store_runtime_string(caller, message);
    let _ = define_host_data_property_from_caller(caller, obj, "message", message_val);
    obj
}

pub(crate) fn obj_proto_to_string_impl(caller: &mut Caller<'_, RuntimeState>, obj: i64) -> i64 {
    if value::is_undefined(obj) {
        store_runtime_string(caller, "[object Undefined]".to_string())
    } else if value::is_null(obj) {
        store_runtime_string(caller, "[object Null]".to_string())
    } else if value::is_array(obj) {
        store_runtime_string(caller, "[object Array]".to_string())
    } else if value::is_function(obj) || value::is_callable(obj) {
        store_runtime_string(caller, "[object Function]".to_string())
    } else if is_promise_value(caller.data(), obj) {
        store_runtime_string(caller, "[object Promise]".to_string())
    } else if value::is_object(obj) {
        let obj_ptr = resolve_handle_idx(
            caller,
            value::decode_object_handle(obj) as usize,
        );
        if let Some(op) = obj_ptr {
            let map_handle = read_object_property_by_name(caller, op, "__map_handle__");
            if map_handle.is_some() {
                return store_runtime_string(caller, "[object Map]".to_string());
            }
            let set_handle = read_object_property_by_name(caller, op, "__set_handle__");
            if set_handle.is_some() {
                return store_runtime_string(caller, "[object Set]".to_string());
            }
        }
        let name_val = obj_ptr
            .and_then(|p| read_object_property_by_name(caller, p, "name"));
        let msg_val = obj_ptr
            .and_then(|p| read_object_property_by_name(caller, p, "message"));
        match (name_val, msg_val) {
            (Some(nv), Some(_mv)) => {
                let name_str = read_value_string_bytes(caller, nv)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                if matches!(
                    name_str.as_str(),
                    "Error"
                        | "TypeError"
                        | "RangeError"
                        | "SyntaxError"
                        | "ReferenceError"
                        | "URIError"
                        | "EvalError"
                        | "AggregateError"
                ) {
                    let obj_ptr2 = resolve_handle_idx(
                        caller,
                        value::decode_object_handle(obj) as usize,
                    );
                    let msg_str = obj_ptr2
                        .and_then(|p| read_object_property_by_name(caller, p, "message"))
                        .and_then(|v| read_value_string_bytes(caller, v))
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default();
                    if msg_str.is_empty() {
                        store_runtime_string(caller, name_str)
                    } else {
                        store_runtime_string(caller, format!("{}: {}", name_str, msg_str))
                    }
                } else {
                    store_runtime_string(caller, "[object Object]".to_string())
                }
            }
            _ => store_runtime_string(caller, "[object Object]".to_string()),
        }
    } else {
        store_runtime_string(caller, "[object Object]".to_string())
    }
}

pub(crate) fn define_host_data_property_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    val: i64,
) -> Option<()> {
    let name_id = find_memory_c_string_global(caller, name)
        .or_else(|| alloc_heap_c_string_global(caller, name))?;
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(obj) as usize)?;
    let (capacity, num_props) = {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        let capacity = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]);
        let num_props = u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]);
        (capacity, num_props)
    };
    if num_props >= capacity {
        return None;
    }
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return None;
    };
    let data = memory.data_mut(&mut *caller);
    let slot_offset = obj_ptr + 16 + num_props as usize * 32;
    if slot_offset + 32 > data.len() {
        return None;
    }
    let flags =
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE;
    let undef = value::encode_undefined();
    data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
    data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
    data[slot_offset + 16..slot_offset + 24].copy_from_slice(&undef.to_le_bytes());
    data[slot_offset + 24..slot_offset + 32].copy_from_slice(&undef.to_le_bytes());
    data[obj_ptr + 12..obj_ptr + 16].copy_from_slice(&(num_props + 1).to_le_bytes());
    Some(())
}

pub(crate) fn alloc_all_settled_result_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    status: &str,
    value_name: &str,
    val: i64,
) -> i64 {
    let obj = alloc_host_object_from_caller(caller, 2);
    let status_value = store_runtime_string(caller, status.to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "status", status_value);
    let _ = define_host_data_property_from_caller(caller, obj, value_name, val);
    obj
}

pub(crate) fn alloc_aggregate_error_from_caller(caller: &mut Caller<'_, RuntimeState>, errors: i64) -> i64 {
    let obj = alloc_host_object_from_caller(caller, 3);
    let name = store_runtime_string(caller, "AggregateError".to_string());
    let message = store_runtime_string(caller, "All promises were rejected".to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "name", name);
    let _ = define_host_data_property_from_caller(caller, obj, "message", message);
    let _ = define_host_data_property_from_caller(caller, obj, "errors", errors);
    obj
}

pub(crate) fn read_string_bytes_from_store(store: &Store<RuntimeState>, memory: &Memory, ptr: u32) -> Vec<u8> {
    let data = memory.data(store);
    let start = ptr as usize;
    if start >= data.len() {
        return Vec::new();
    }
    let end = data[start..]
        .iter()
        .position(|byte| *byte == 0)
        .map_or(data.len(), |offset| start + offset);
    data[start..end].to_vec()
}

pub(crate) fn find_memory_c_string_from_store(
    store: &Store<RuntimeState>,
    memory: &Memory,
    name: &str,
) -> Option<u32> {
    let mut needle = Vec::with_capacity(name.len() + 1);
    needle.extend_from_slice(name.as_bytes());
    needle.push(0);
    memory
        .data(store)
        .windows(needle.len())
        .position(|window| window == needle.as_slice())
        .map(|offset| offset as u32)
}

pub(crate) fn alloc_heap_c_string_from_store(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    heap_ptr_global: &Global,
    name: &str,
) -> Option<u32> {
    let heap_ptr = heap_ptr_global.get(&mut *store).i32().unwrap_or(0) as usize;
    let bytes = name.as_bytes();
    let end = heap_ptr.checked_add(bytes.len() + 1)?;
    let aligned_end = (end + 7) & !7;
    {
        let data = memory.data_mut(&mut *store);
        if aligned_end > data.len() {
            return None;
        }
        data[heap_ptr..heap_ptr + bytes.len()].copy_from_slice(bytes);
        data[heap_ptr + bytes.len()] = 0;
        data[end..aligned_end].fill(0);
    }
    let _ = heap_ptr_global.set(&mut *store, Val::I32(aligned_end as i32));
    Some(heap_ptr as u32)
}

pub(crate) fn resolve_handle_idx_from_store(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    obj_table_ptr_global: &Global,
    handle_idx: usize,
) -> Option<usize> {
    let obj_table_ptr = obj_table_ptr_global.get(&mut *store).i32().unwrap_or(0) as usize;
    let slot_addr = obj_table_ptr + handle_idx * 4;
    let data = memory.data(&mut *store);
    if slot_addr + 4 > data.len() {
        return None;
    }
    let ptr = u32::from_le_bytes([
        data[slot_addr],
        data[slot_addr + 1],
        data[slot_addr + 2],
        data[slot_addr + 3],
    ]) as usize;
    if ptr == 0 { None } else { Some(ptr) }
}

pub(crate) fn resolve_handle_from_store(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    obj_table_ptr_global: &Global,
    val: i64,
) -> Option<usize> {
    let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
    resolve_handle_idx_from_store(store, memory, obj_table_ptr_global, handle_idx)
}

pub(crate) fn read_object_property_by_name_from_store(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    obj_ptr: usize,
    prop_name: &str,
) -> Option<i64> {
    let num_props = {
        let data = memory.data(&mut *store);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]) as usize
    };
    let mut name_ids = Vec::with_capacity(num_props);
    {
        let data = memory.data(&mut *store);
        for i in 0..num_props {
            let slot_offset = obj_ptr + 16 + i * 32;
            if slot_offset + 4 > data.len() {
                break;
            }
            name_ids.push(u32::from_le_bytes([
                data[slot_offset],
                data[slot_offset + 1],
                data[slot_offset + 2],
                data[slot_offset + 3],
            ]));
        }
    }
    for (index, name_id) in name_ids.iter().enumerate() {
        if read_string_bytes_from_store(store, memory, *name_id) == prop_name.as_bytes() {
            let data = memory.data(&mut *store);
            let slot_offset = obj_ptr + 16 + index * 32;
            if slot_offset + 16 > data.len() {
                return None;
            }
            return Some(i64::from_le_bytes([
                data[slot_offset + 8],
                data[slot_offset + 9],
                data[slot_offset + 10],
                data[slot_offset + 11],
                data[slot_offset + 12],
                data[slot_offset + 13],
                data[slot_offset + 14],
                data[slot_offset + 15],
            ]));
        }
    }
    None
}

pub(crate) fn alloc_host_object_from_store(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    heap_ptr_global: &Global,
    obj_table_ptr_global: &Global,
    obj_table_count_global: &Global,
    capacity: u32,
) -> i64 {
    let heap_ptr = heap_ptr_global.get(&mut *store).i32().unwrap_or(0) as u32;
    let obj_table_count = obj_table_count_global.get(&mut *store).i32().unwrap_or(0) as u32;
    let obj_table_ptr = obj_table_ptr_global.get(&mut *store).i32().unwrap_or(0) as u32;
    let size = 16 + capacity * 32;
    let new_heap_ptr = heap_ptr.saturating_add(size);
    {
        let data = memory.data_mut(&mut *store);
        let ptr = heap_ptr as usize;
        if new_heap_ptr as usize > data.len() {
            return value::encode_undefined();
        }
        data[ptr..ptr + 4].copy_from_slice(&0u32.to_le_bytes());
        data[ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT;
        data[ptr + 5..ptr + 8].fill(0);
        data[ptr + 8..ptr + 12].copy_from_slice(&capacity.to_le_bytes());
        data[ptr + 12..ptr + 16].copy_from_slice(&0u32.to_le_bytes());
        let slot_addr = (obj_table_ptr + obj_table_count * 4) as usize;
        if slot_addr + 4 <= data.len() {
            data[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
        }
    }
    let _ = heap_ptr_global.set(&mut *store, Val::I32(new_heap_ptr as i32));
    let _ = obj_table_count_global.set(&mut *store, Val::I32((obj_table_count + 1) as i32));
    value::encode_object_handle(obj_table_count)
}

pub(crate) fn write_array_elem_from_store(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    ptr: usize,
    index: u32,
    val: i64,
) {
    let data = memory.data_mut(&mut *store);
    let elem_offset = ptr + 16 + index as usize * 8;
    if elem_offset + 8 <= data.len() {
        data[elem_offset..elem_offset + 8].copy_from_slice(&val.to_le_bytes());
    }
}

pub(crate) fn define_host_data_property_from_store(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    heap_ptr_global: &Global,
    obj_table_ptr_global: &Global,
    obj: i64,
    name: &str,
    val: i64,
) -> Option<()> {
    let name_id = find_memory_c_string_from_store(store, memory, name)
        .or_else(|| alloc_heap_c_string_from_store(store, memory, heap_ptr_global, name))?;
    let obj_ptr = resolve_handle_idx_from_store(
        store,
        memory,
        obj_table_ptr_global,
        value::decode_object_handle(obj) as usize,
    )?;
    let (capacity, num_props) = {
        let data = memory.data(&mut *store);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        let capacity = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]);
        let num_props = u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]);
        (capacity, num_props)
    };
    if num_props >= capacity {
        return None;
    }
    let data = memory.data_mut(&mut *store);
    let slot_offset = obj_ptr + 16 + num_props as usize * 32;
    if slot_offset + 32 > data.len() {
        return None;
    }
    let flags =
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE;
    let undef = value::encode_undefined();
    data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
    data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
    data[slot_offset + 16..slot_offset + 24].copy_from_slice(&undef.to_le_bytes());
    data[slot_offset + 24..slot_offset + 32].copy_from_slice(&undef.to_le_bytes());
    data[obj_ptr + 12..obj_ptr + 16].copy_from_slice(&(num_props + 1).to_le_bytes());
    Some(())
}

pub(crate) fn alloc_all_settled_result_from_store(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    heap_ptr_global: &Global,
    obj_table_ptr_global: &Global,
    obj_table_count_global: &Global,
    status: &str,
    value_name: &str,
    val: i64,
) -> i64 {
    let obj = alloc_host_object_from_store(
        store,
        memory,
        heap_ptr_global,
        obj_table_ptr_global,
        obj_table_count_global,
        2,
    );
    let status_value = store_runtime_string_in_state(store.data(), status.to_string());
    let _ = define_host_data_property_from_store(
        store,
        memory,
        heap_ptr_global,
        obj_table_ptr_global,
        obj,
        "status",
        status_value,
    );
    let _ = define_host_data_property_from_store(
        store,
        memory,
        heap_ptr_global,
        obj_table_ptr_global,
        obj,
        value_name,
        val,
    );
    obj
}

pub(crate) fn alloc_aggregate_error_from_store(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    heap_ptr_global: &Global,
    obj_table_ptr_global: &Global,
    obj_table_count_global: &Global,
    errors: i64,
) -> i64 {
    let obj = alloc_host_object_from_store(
        store,
        memory,
        heap_ptr_global,
        obj_table_ptr_global,
        obj_table_count_global,
        3,
    );
    let name = store_runtime_string_in_state(store.data(), "AggregateError".to_string());
    let message =
        store_runtime_string_in_state(store.data(), "All promises were rejected".to_string());
    let _ = define_host_data_property_from_store(
        store,
        memory,
        heap_ptr_global,
        obj_table_ptr_global,
        obj,
        "name",
        name,
    );
    let _ = define_host_data_property_from_store(
        store,
        memory,
        heap_ptr_global,
        obj_table_ptr_global,
        obj,
        "message",
        message,
    );
    let _ = define_host_data_property_from_store(
        store,
        memory,
        heap_ptr_global,
        obj_table_ptr_global,
        obj,
        "errors",
        errors,
    );
    obj
}

/// GC 标记阶段：递归标记对象及其所有可达对象。
/// 使用标记位图避免重复标记和循环引用。
pub(crate) fn mark_object_recursive(
    caller: &mut Caller<'_, RuntimeState>,
    handle_idx: usize,
    obj_ptr: usize,
    obj_table_ptr: usize,
    obj_table_count: usize,
) {
    // 检查标记位图
    let word_idx = handle_idx / 64;
    let bit_idx = handle_idx % 64;

    {
        let mut mark_bits = caller
            .data()
            .gc_mark_bits
            .lock()
            .expect("gc_mark_bits mutex");
        if word_idx >= mark_bits.len() {
            // 扩展位图
            mark_bits.resize(word_idx + 1, 0);
        }
        // 已标记，跳过
        if (mark_bits[word_idx] & (1u64 << bit_idx)) != 0 {
            return;
        }
        // 标记
        mark_bits[word_idx] |= 1u64 << bit_idx;
    }

    // 收集需要递归标记的对象列表
    let mut children_to_mark: Vec<(usize, usize)> = Vec::new(); // (handle_idx, obj_ptr)

    // 获取内存并读取信息
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return;
        };
        let data = memory.data(&*caller);

        // 读取对象头 — 至少需要 16 字节（proto + type + pad + capacity/length + num_props/capacity）
        if obj_ptr + 16 > data.len() {
            return;
        }

        // 读取 proto_handle（offset 0，未改变）
        let proto_handle = u32::from_le_bytes([
            data[obj_ptr],
            data[obj_ptr + 1],
            data[obj_ptr + 2],
            data[obj_ptr + 3],
        ]);
        if proto_handle != 0xFFFF_FFFF && (proto_handle as usize) < obj_table_count {
            let proto_slot_addr = obj_table_ptr + proto_handle as usize * 4;
            if proto_slot_addr + 4 <= data.len() {
                let proto_ptr = u32::from_le_bytes([
                    data[proto_slot_addr],
                    data[proto_slot_addr + 1],
                    data[proto_slot_addr + 2],
                    data[proto_slot_addr + 3],
                ]) as usize;
                if proto_ptr != 0 {
                    children_to_mark.push((proto_handle as usize, proto_ptr));
                }
            }
        }

        // 读取 type byte 决定是数组还是对象
        let heap_type = data[obj_ptr + 4];

        if heap_type == wjsm_ir::HEAP_TYPE_ARRAY {
            // ── 数组对象 ──
            let len = u32::from_le_bytes([
                data[obj_ptr + 8],
                data[obj_ptr + 9],
                data[obj_ptr + 10],
                data[obj_ptr + 11],
            ]) as usize;

            // 迭代 8 字节元素，收集对象/函数引用
            for i in 0..len {
                let elem_offset = obj_ptr + 16 + i * 8;
                if elem_offset + 8 > data.len() {
                    break;
                }
                let elem = i64::from_le_bytes([
                    data[elem_offset],
                    data[elem_offset + 1],
                    data[elem_offset + 2],
                    data[elem_offset + 3],
                    data[elem_offset + 4],
                    data[elem_offset + 5],
                    data[elem_offset + 6],
                    data[elem_offset + 7],
                ]);
                if value::is_object(elem) || value::is_function(elem) {
                    let child_handle_idx = (elem as u64 & 0xFFFF_FFFF) as usize;
                    if child_handle_idx < obj_table_count {
                        let child_slot_addr = obj_table_ptr + child_handle_idx * 4;
                        if child_slot_addr + 4 <= data.len() {
                            let child_ptr = u32::from_le_bytes([
                                data[child_slot_addr],
                                data[child_slot_addr + 1],
                                data[child_slot_addr + 2],
                                data[child_slot_addr + 3],
                            ]) as usize;
                            if child_ptr != 0 {
                                children_to_mark.push((child_handle_idx, child_ptr));
                            }
                        }
                    }
                }
            }
        } else {
            // ── 普通对象 ──
            let num_props = u32::from_le_bytes([
                data[obj_ptr + 12],
                data[obj_ptr + 13],
                data[obj_ptr + 14],
                data[obj_ptr + 15],
            ]) as usize;

            // 遍历属性，收集所有对象/函数引用
            // 属性槽: [name_id(4), flags(4), value(8), getter(8), setter(8)] = 32 字节
            // 属性槽起始: ptr + 16
            for i in 0..num_props {
                let slot_offset = obj_ptr + 16 + i * 32;
                if slot_offset + 32 > data.len() {
                    break;
                }

                // 读取 value (offset 8), getter (offset 16), setter (offset 24)
                let value = i64::from_le_bytes([
                    data[slot_offset + 8],
                    data[slot_offset + 9],
                    data[slot_offset + 10],
                    data[slot_offset + 11],
                    data[slot_offset + 12],
                    data[slot_offset + 13],
                    data[slot_offset + 14],
                    data[slot_offset + 15],
                ]);
                let getter = i64::from_le_bytes([
                    data[slot_offset + 16],
                    data[slot_offset + 17],
                    data[slot_offset + 18],
                    data[slot_offset + 19],
                    data[slot_offset + 20],
                    data[slot_offset + 21],
                    data[slot_offset + 22],
                    data[slot_offset + 23],
                ]);
                let setter = i64::from_le_bytes([
                    data[slot_offset + 24],
                    data[slot_offset + 25],
                    data[slot_offset + 26],
                    data[slot_offset + 27],
                    data[slot_offset + 28],
                    data[slot_offset + 29],
                    data[slot_offset + 30],
                    data[slot_offset + 31],
                ]);

                for val in [value, getter, setter] {
                    if value::is_object(val) || value::is_function(val) {
                        let child_handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
                        if child_handle_idx < obj_table_count {
                            let child_slot_addr = obj_table_ptr + child_handle_idx * 4;
                            if child_slot_addr + 4 <= data.len() {
                                let child_ptr = u32::from_le_bytes([
                                    data[child_slot_addr],
                                    data[child_slot_addr + 1],
                                    data[child_slot_addr + 2],
                                    data[child_slot_addr + 3],
                                ]) as usize;
                                if child_ptr != 0 {
                                    children_to_mark.push((child_handle_idx, child_ptr));
                                }
                            }
                        }
                    }
                }
            }
        }
    } // data 借用在这里结束

    // 递归标记收集到的对象
    for (child_handle_idx, child_ptr) in children_to_mark {
        mark_object_recursive(
            caller,
            child_handle_idx,
            child_ptr,
            obj_table_ptr,
            obj_table_count,
        );
    }
}

/// 通过 handle 表解析 boxed value 的真实对象指针。
/// 支持 TAG_OBJECT 和 TAG_FUNCTION（统一走 handle 表）。
pub(crate) fn resolve_handle(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Option<usize> {
    let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
    resolve_handle_idx(caller, handle_idx)
}

pub(crate) fn same_value_zero(a: i64, b: i64) -> bool {
    if a == b {
        return true;
    }
    if value::is_f64(a) && value::is_f64(b) {
        let fa = value::decode_f64(a);
        let fb = value::decode_f64(b);
        if fa.is_nan() && fb.is_nan() {
            return true;
        }
        if fa == 0.0 && fb == 0.0 {
            return true;
        }
    }
    false
}

/// 通过 handle_idx 解析真实对象指针。
pub(crate) fn resolve_handle_idx(caller: &mut Caller<'_, RuntimeState>, handle_idx: usize) -> Option<usize> {
    let obj_table_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else {
            return None;
        };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };
    let slot_addr = obj_table_ptr + handle_idx * 4;
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return None;
    };
    let d = mem.data(&*caller);
    if slot_addr + 4 > d.len() {
        return None;
    }
    let ptr = u32::from_le_bytes([
        d[slot_addr],
        d[slot_addr + 1],
        d[slot_addr + 2],
        d[slot_addr + 3],
    ]) as usize;
    if ptr == 0 {
        return None;
    }
    Some(ptr)
}

// ── Array helpers ──────────────────────────────────────────────────────

/// 解析 TAG_ARRAY 值 → 数组对象的内存指针
pub(crate) fn resolve_array_ptr(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Option<usize> {
    let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
    resolve_handle_idx(caller, handle_idx)
}

/// 读取数组的 length 字段（offset 8）

pub(crate) fn alloc_array(caller: &mut Caller<'_, RuntimeState>, capacity: u32) -> i64 {
    let heap_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
            return value::encode_undefined();
        };
        g.get(&mut *caller).i32().unwrap_or(0) as u32
    };
    let obj_table_count = {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") else {
            return value::encode_undefined();
        };
        g.get(&mut *caller).i32().unwrap_or(0) as u32
    };
    let obj_table_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else {
            return value::encode_undefined();
        };
        g.get(&mut *caller).i32().unwrap_or(0) as u32
    };
    let size = 16 + capacity * 8;
    let new_heap_ptr = heap_ptr + size;
    if let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") {
        let _ = g.set(&mut *caller, Val::I32(new_heap_ptr as i32));
    }
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return value::encode_undefined();
    };
    let d = mem.data_mut(&mut *caller);
    let ptr = heap_ptr as usize;
    if (new_heap_ptr as usize) > d.len() {
        return value::encode_undefined();
    }
    d[ptr..ptr + 4].copy_from_slice(&(-1i32).to_le_bytes());
    d[ptr + 4] = 1u8;
    d[ptr + 5..ptr + 8].fill(0);
    d[ptr + 8..ptr + 12].copy_from_slice(&0u32.to_le_bytes());
    d[ptr + 12..ptr + 16].copy_from_slice(&capacity.to_le_bytes());
    let slot_addr = (obj_table_ptr + obj_table_count * 4) as usize;
    if slot_addr + 4 <= d.len() {
        d[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
    }
    let _ = d;
    if let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") {
        let _ = g.set(&mut *caller, Val::I32((obj_table_count + 1) as i32));
    }
    value::encode_handle(value::TAG_ARRAY, obj_table_count)
}

pub(crate) fn alloc_object(caller: &mut Caller<'_, RuntimeState>, capacity: u32) -> i64 {
    let heap_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
            return value::encode_undefined();
        };
        g.get(&mut *caller).i32().unwrap_or(0) as u32
    };
    let obj_table_count = {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") else {
            return value::encode_undefined();
        };
        g.get(&mut *caller).i32().unwrap_or(0) as u32
    };
    let obj_table_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else {
            return value::encode_undefined();
        };
        g.get(&mut *caller).i32().unwrap_or(0) as u32
    };
    let size = 16 + capacity * 32;
    let new_heap_ptr = heap_ptr + size;
    if let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") {
        let _ = g.set(&mut *caller, Val::I32(new_heap_ptr as i32));
    }
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return value::encode_undefined();
    };
    let d = mem.data_mut(&mut *caller);
    let ptr = heap_ptr as usize;
    if (new_heap_ptr as usize) > d.len() {
        return value::encode_undefined();
    }
    d[ptr..ptr + 4].copy_from_slice(&0u32.to_le_bytes());
    d[ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT;
    d[ptr + 5..ptr + 8].fill(0);
    d[ptr + 8..ptr + 12].copy_from_slice(&capacity.to_le_bytes());
    d[ptr + 12..ptr + 16].copy_from_slice(&0u32.to_le_bytes());
    let slot_addr = (obj_table_ptr + obj_table_count * 4) as usize;
    if slot_addr + 4 <= d.len() {
        d[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
    }
    let _ = d;
    if let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") {
        let _ = g.set(&mut *caller, Val::I32((obj_table_count + 1) as i32));
    }
    value::encode_object_handle(obj_table_count)
}

pub(crate) fn find_memory_c_string(caller: &mut Caller<'_, RuntimeState>, name: &str) -> Option<u32> {
    let mut needle = Vec::with_capacity(name.len() + 1);
    needle.extend_from_slice(name.as_bytes());
    needle.push(0);
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return None;
    };
    memory
        .data(&*caller)
        .windows(needle.len())
        .position(|window| window == needle.as_slice())
        .map(|offset| offset as u32)
}

pub(crate) fn alloc_heap_c_string(caller: &mut Caller<'_, RuntimeState>, name: &str) -> Option<u32> {
    let heap_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
            return None;
        };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };
    let bytes = name.as_bytes();
    let end = heap_ptr.checked_add(bytes.len() + 1)?;
    let aligned_end = (end + 7) & !7;
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data_mut(&mut *caller);
        if aligned_end > data.len() {
            return None;
        }
        data[heap_ptr..heap_ptr + bytes.len()].copy_from_slice(bytes);
        data[heap_ptr + bytes.len()] = 0;
        data[end..aligned_end].fill(0);
    }
    if let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") {
        let _ = g.set(&mut *caller, Val::I32(aligned_end as i32));
    }
    Some(heap_ptr as u32)
}

pub(crate) fn define_host_data_property(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    val: i64,
) -> Option<()> {
    let name_id =
        find_memory_c_string(caller, name).or_else(|| alloc_heap_c_string(caller, name))?;
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(obj) as usize)?;
    let (capacity, num_props) = {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        let capacity = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]);
        let num_props = u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]);
        (capacity, num_props)
    };
    let actual_ptr = if num_props >= capacity {
        let new_cap = capacity.saturating_mul(2).max(num_props + 1).max(1);
        grow_object(caller, obj_ptr, obj, new_cap)?
    } else {
        obj_ptr
    };
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return None;
    };
    let data = memory.data_mut(&mut *caller);
    let slot_offset = actual_ptr + 16 + num_props as usize * 32;
    if slot_offset + 32 > data.len() {
        return None;
    }
    let flags =
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE;
    let undef = value::encode_undefined();
    data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
    data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
    data[slot_offset + 16..slot_offset + 24].copy_from_slice(&undef.to_le_bytes());
    data[slot_offset + 24..slot_offset + 32].copy_from_slice(&undef.to_le_bytes());
    data[actual_ptr + 12..actual_ptr + 16].copy_from_slice(&(num_props + 1).to_le_bytes());
    Some(())
}

pub(crate) fn alloc_promise_all_settled_result(
    caller: &mut Caller<'_, RuntimeState>,
    status: &str,
    value_name: &str,
    value: i64,
) -> i64 {
    let obj = alloc_object(caller, 2);
    let _ = define_host_data_property(
        caller,
        obj,
        "status",
        store_runtime_string(caller, status.to_string()),
    );
    let _ = define_host_data_property(caller, obj, value_name, value);
    obj
}

pub(crate) fn alloc_aggregate_error(caller: &mut Caller<'_, RuntimeState>, errors: i64) -> i64 {
    let obj = alloc_object(caller, 3);
    let name = store_runtime_string(caller, "AggregateError".to_string());
    let message = store_runtime_string(caller, "All promises were rejected".to_string());
    let _ = define_host_data_property(caller, obj, "name", name);
    let _ = define_host_data_property(caller, obj, "message", message);
    let _ = define_host_data_property(caller, obj, "errors", errors);
    obj
}
