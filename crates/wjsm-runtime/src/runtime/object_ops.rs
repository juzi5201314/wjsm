use wjsm_ir::{constants, value};
use wasmtime::{Caller, Extern, Val};

use crate::types::RuntimeState;
use crate::runtime::string_utils::{read_string_bytes, store_runtime_string};
use crate::runtime::memory::{resolve_handle_idx, resolve_handle};

pub(crate) fn grow_object(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
    handle_val: i64,
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
    let new_size = 16 + new_cap as usize * 32;
    let old_cap = {
        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
            return None;
        };
        let d = mem.data(&*caller);
        if ptr + 12 > d.len() {
            return None;
        }
        u32::from_le_bytes([d[ptr + 8], d[ptr + 9], d[ptr + 10], d[ptr + 11]]) as usize
    };
    let old_size = 16 + old_cap * 32;
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return None;
    };
    let d = mem.data_mut(&mut *caller);
    if heap_ptr + new_size > d.len() {
        return None;
    }
    d.copy_within(ptr..ptr + old_size, heap_ptr);
    d[heap_ptr + 8..heap_ptr + 12].copy_from_slice(&new_cap.to_le_bytes());
    let handle_idx = (handle_val as u64 & 0xFFFF_FFFF) as usize;
    let slot_addr = obj_table_ptr + handle_idx * 4;
    if slot_addr + 4 <= d.len() {
        d[slot_addr..slot_addr + 4].copy_from_slice(&(heap_ptr as u32).to_le_bytes());
    }
    if let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") {
        let _ = g.set(&mut *caller, Val::I32((heap_ptr + new_size) as i32));
    }
    Some(heap_ptr)
}

/// 自顶向下迭代 merge sort，稳定 O(n log n)
pub(crate) fn merge_sort_by<T: Copy>(slice: &mut [T], cmp: &mut dyn FnMut(&T, &T) -> std::cmp::Ordering) {
    let n = slice.len();
    if n <= 1 {
        return;
    }
    let mut buf: Vec<T> = Vec::with_capacity(n);
    let mut width = 1;
    while width < n {
        let mut i = 0;
        while i < n {
            let mid = (i + width).min(n);
            let end = (i + 2 * width).min(n);
            let mut left = i;
            let mut right = mid;
            while left < mid && right < end {
                if cmp(&slice[left], &slice[right]) != std::cmp::Ordering::Greater {
                    buf.push(slice[left]);
                    left += 1;
                } else {
                    buf.push(slice[right]);
                    right += 1;
                }
            }
            while left < mid {
                buf.push(slice[left]);
                left += 1;
            }
            while right < end {
                buf.push(slice[right]);
                right += 1;
            }
            slice[i..end].copy_from_slice(&buf[..end - i]);
            buf.clear();
            i += 2 * width;
        }
        width *= 2;
    }
}

/// 从对象中按名称读取属性值（用于 define_property 等）
pub(crate) fn read_object_property_by_name(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    prop_name: &str,
) -> Option<i64> {
    let num_props = {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
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
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
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
    for (i, name_id) in name_ids.iter().enumerate() {
        let name_bytes = read_string_bytes(caller, *name_id);
        if name_bytes == prop_name.as_bytes() {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return None;
            };
            let data = memory.data(&*caller);
            let slot_offset = obj_ptr + 16 + i * 32;
            if slot_offset + 32 > data.len() {
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

/// 从对象中按 name_id 查找属性的 slot_offset
pub(crate) fn find_property_slot_by_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    name_id: u32,
) -> Option<(usize, i32, i64)> {
    let num_props = {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
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
    let target_name_bytes = read_string_bytes(caller, name_id);
    for i in 0..num_props {
        let slot_offset = obj_ptr + 16 + i * 32;
        let (slot_name_id, flags, val) = {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return None;
            };
            let data = memory.data(&*caller);
            if slot_offset + 32 > data.len() {
                break;
            }
            let slot_name_id = u32::from_le_bytes([
                data[slot_offset],
                data[slot_offset + 1],
                data[slot_offset + 2],
                data[slot_offset + 3],
            ]);
            let flags = i32::from_le_bytes([
                data[slot_offset + 4],
                data[slot_offset + 5],
                data[slot_offset + 6],
                data[slot_offset + 7],
            ]);
            let val = i64::from_le_bytes([
                data[slot_offset + 8],
                data[slot_offset + 9],
                data[slot_offset + 10],
                data[slot_offset + 11],
                data[slot_offset + 12],
                data[slot_offset + 13],
                data[slot_offset + 14],
                data[slot_offset + 15],
            ]);
            (slot_name_id, flags, val)
        };
        let same_name = slot_name_id == name_id
            || (!target_name_bytes.is_empty()
                && read_string_bytes(caller, slot_name_id) == target_name_bytes);
        if same_name {
            return Some((slot_offset, flags, val));
        }
    }
    None
}

pub(crate) fn read_object_property_by_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    name_id: u32,
) -> Option<i64> {
    let (slot_offset, _flags, val) = find_property_slot_by_name_id(caller, obj_ptr, name_id)?;
    let _ = slot_offset;
    Some(val)
}

pub(crate) fn write_object_property_by_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    obj_handle: i64,
    name_id: u32,
    val: i64,
    flags: i32,
) {
    let found = find_property_slot_by_name_id(caller, obj_ptr, name_id);
    if let Some((slot_offset, _, _)) = found {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return;
        };
        let data = memory.data_mut(&mut *caller);
        data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
        let _ = flags;
    } else {
        let (num_props, capacity) = {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return;
            };
            let data = memory.data(&*caller);
            if obj_ptr + 16 > data.len() {
                return;
            }
            let cap = u32::from_le_bytes([
                data[obj_ptr + 8],
                data[obj_ptr + 9],
                data[obj_ptr + 10],
                data[obj_ptr + 11],
            ]) as usize;
            let num = u32::from_le_bytes([
                data[obj_ptr + 12],
                data[obj_ptr + 13],
                data[obj_ptr + 14],
                data[obj_ptr + 15],
            ]) as usize;
            (num, cap)
        };
        if num_props >= capacity {
            let new_cap = std::cmp::max(capacity * 2, 4) as u32;
            let _ = grow_object(caller, obj_ptr, obj_handle, new_cap);
        }
        let num_props = {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return;
            };
            let data = memory.data(&*caller);
            if obj_ptr + 16 > data.len() {
                return;
            }
            u32::from_le_bytes([
                data[obj_ptr + 12],
                data[obj_ptr + 13],
                data[obj_ptr + 14],
                data[obj_ptr + 15],
            ]) as usize
        };
        let new_count = num_props + 1;
        let slot_offset = obj_ptr + 16 + num_props * 32;
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return;
        };
        let data = memory.data_mut(&mut *caller);
        if slot_offset + 32 > data.len() {
            return;
        }
        data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
        data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
        data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
        data[obj_ptr + 12..obj_ptr + 16].copy_from_slice(&(new_count as u32).to_le_bytes());
    }
}

/// 读取对象/函数的所有属性名，用于 for...in 枚举
pub(crate) fn enumerate_object_keys(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Vec<String> {
    // 解析对象指针：通过 handle 表统一解析
    let ptr: usize = match resolve_handle(caller, val) {
        Some(p) => p,
        None => return Vec::new(),
    };

    // 读取属性列表
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Vec::new();
    };
    let data = memory.data(&*caller);
    if ptr + 16 > data.len() {
        return Vec::new();
    }

    let num_props = u32::from_le_bytes([
        data[ptr + 12],
        data[ptr + 13],
        data[ptr + 14],
        data[ptr + 15],
    ]) as usize;

    let mut name_ids = Vec::with_capacity(num_props);
    for i in 0..num_props {
        let slot_offset = ptr + 16 + i * 32;
        if slot_offset + 4 > data.len() {
            break;
        }
        let name_id = u32::from_le_bytes([
            data[slot_offset],
            data[slot_offset + 1],
            data[slot_offset + 2],
            data[slot_offset + 3],
        ]);
        name_ids.push(name_id);
    }
    let _ = data; // 释放对 memory 的借用

    let mut keys = Vec::with_capacity(name_ids.len());
    for name_id in name_ids {
        let name_bytes = read_string_bytes(caller, name_id);
        if let Ok(name) = std::str::from_utf8(&name_bytes) {
            keys.push(name.to_string());
        }
    }
    keys
}

/// 分配描述符对象，用于 Object.getOwnPropertyDescriptor 返回值
/// 对象格式：header(16 bytes) + 4 slots * 32 bytes = 144 bytes
pub(crate) fn allocate_descriptor_object(
    caller: &mut Caller<'_, RuntimeState>,
    is_accessor: bool,
    value: i64,
    writable: bool,
    enumerable: bool,
    configurable: bool,
    getter: i64,
    setter: i64,
) -> Option<i64> {
    // 读取全局变量
    let obj_table_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else {
            return None;
        };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };
    let obj_table_count = {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") else {
            return None;
        };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };
    let heap_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
            return None;
        };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };

    // 对象大小：16 (header) + 4 * 32 (slots) = 144 bytes
    let obj_size = 16 + 4 * 32;
    let handle_idx = obj_table_count;

    // 分配对象
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data_mut(&mut *caller);
        if heap_ptr + obj_size > data.len() {
            return None;
        }

        // 初始化 header: proto=0, type=OBJECT, pad=0, capacity=4, num_props=0
        data[heap_ptr..heap_ptr + 4].copy_from_slice(&0u32.to_le_bytes()); // proto
        data[heap_ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT; // type byte
        data[heap_ptr + 5..heap_ptr + 8].fill(0); // pad bytes
        data[heap_ptr + 8..heap_ptr + 12].copy_from_slice(&4u32.to_le_bytes()); // capacity
        data[heap_ptr + 12..heap_ptr + 16].copy_from_slice(&0u32.to_le_bytes()); // num_props

        // 注册到 handle 表
        let slot_addr = obj_table_ptr + handle_idx * 4;
        if slot_addr + 4 <= data.len() {
            data[slot_addr..slot_addr + 4].copy_from_slice(&(heap_ptr as u32).to_le_bytes());
        }
    }

    // 更新 __heap_ptr 和 __obj_table_count
    {
        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
            return None;
        };
        let _ = g.set(&mut *caller, Val::I32((heap_ptr + obj_size) as i32));
    }
    {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") else {
            return None;
        };
        let _ = g.set(&mut *caller, Val::I32((handle_idx + 1) as i32));
    }

    // 现在设置描述符对象的属性
    let desc_ptr = heap_ptr;

    // 写入属性的辅助闭包
    let mut write_property = |name_id: u32, val: i64, flags: i32| -> Option<()> {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data_mut(&mut *caller);
        // 读取当前 num_props
        let num_props = u32::from_le_bytes([
            data[desc_ptr + 12],
            data[desc_ptr + 13],
            data[desc_ptr + 14],
            data[desc_ptr + 15],
        ]) as usize;
        let slot_offset = desc_ptr + 16 + num_props * 32;
        if slot_offset + 32 > data.len() {
            return None;
        }
        data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
        data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
        data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
        // getter 和 setter 为 undefined
        let undef = value::encode_undefined();
        data[slot_offset + 16..slot_offset + 24].copy_from_slice(&undef.to_le_bytes());
        data[slot_offset + 24..slot_offset + 32].copy_from_slice(&undef.to_le_bytes());
        // 更新 num_props
        let new_num_props = (num_props + 1) as u32;
        data[desc_ptr + 12..desc_ptr + 16].copy_from_slice(&new_num_props.to_le_bytes());
        Some(())
    };

    // flags: enumerable 和 configurable
    let base_flags: i32 =
        (if enumerable { 1 << 1 } else { 0 }) | (if configurable { 1 } else { 0 });

    if is_accessor {
        // 访问器属性：get, set, enumerable, configurable
        // writable flag 不适用于访问器属性
        let get_flags = base_flags | (1 << 2); // writable=true for function values
        write_property(constants::PROP_DESC_GET_OFFSET, getter, get_flags)?;
        write_property(constants::PROP_DESC_SET_OFFSET, setter, get_flags)?;
    } else {
        // 数据属性：value, writable, enumerable, configurable
        let writable_flags = base_flags | (if writable { 1 << 2 } else { 0 });
        write_property(constants::PROP_DESC_VALUE_OFFSET, value, writable_flags)?;
        write_property(
            constants::PROP_DESC_WRITABLE_OFFSET,
            value::encode_bool(writable),
            base_flags | (1 << 2),
        )?;
    }

    // enumerable 和 configurable 对于两种属性都要写
    write_property(
        constants::PROP_DESC_ENUMERABLE_OFFSET,
        value::encode_bool(enumerable),
        base_flags | (1 << 2),
    )?;
    write_property(
        constants::PROP_DESC_CONFIGURABLE_OFFSET,
        value::encode_bool(configurable),
        base_flags | (1 << 2),
    )?;

    // 返回对象 handle
    Some(value::encode_object_handle(handle_idx as u32))
}

// ── 辅助函数用于 abstract_eq 和 abstract_compare ─────────────────────────

/// ToNumber 抽象操作 (ECMAScript 7.1.4)
/// 将值转换为 Number 类型

pub(crate) fn collect_own_property_names(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    enumerable_only: bool,
) -> Vec<String> {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return vec![];
    };
    let data = mem.data(&*caller);
    if obj_ptr + 16 > data.len() {
        return vec![];
    }
    let num_props = u32::from_le_bytes([
        data[obj_ptr + 12],
        data[obj_ptr + 13],
        data[obj_ptr + 14],
        data[obj_ptr + 15],
    ]) as usize;
    let mut name_ids = Vec::new();
    for i in 0..num_props {
        let slot_offset = obj_ptr + 16 + i * 32;
        if slot_offset + 32 > data.len() {
            break;
        }
        let flags = i32::from_le_bytes([
            data[slot_offset + 4],
            data[slot_offset + 5],
            data[slot_offset + 6],
            data[slot_offset + 7],
        ]);
        if enumerable_only && (flags & 2) == 0 {
            continue;
        }
        let name_id = u32::from_le_bytes([
            data[slot_offset],
            data[slot_offset + 1],
            data[slot_offset + 2],
            data[slot_offset + 3],
        ]);
        name_ids.push(name_id);
    }
    let _ = data;
    let _ = mem;
    let mut names = Vec::new();
    for name_id in name_ids {
        let name_bytes = read_string_bytes(caller, name_id);
        names.push(String::from_utf8_lossy(&name_bytes).to_string());
    }
    names
}

pub(crate) fn collect_own_property_values(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    enumerable_only: bool,
) -> Vec<i64> {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return vec![];
    };
    let data = mem.data(&*caller);
    if obj_ptr + 16 > data.len() {
        return vec![];
    }
    let num_props = u32::from_le_bytes([
        data[obj_ptr + 12],
        data[obj_ptr + 13],
        data[obj_ptr + 14],
        data[obj_ptr + 15],
    ]) as usize;
    let mut values = Vec::new();
    for i in 0..num_props {
        let slot_offset = obj_ptr + 16 + i * 32;
        if slot_offset + 32 > data.len() {
            break;
        }
        let flags = i32::from_le_bytes([
            data[slot_offset + 4],
            data[slot_offset + 5],
            data[slot_offset + 6],
            data[slot_offset + 7],
        ]);
        if enumerable_only && (flags & 2) == 0 {
            continue;
        }
        let val = i64::from_le_bytes([
            data[slot_offset + 8],
            data[slot_offset + 9],
            data[slot_offset + 10],
            data[slot_offset + 11],
            data[slot_offset + 12],
            data[slot_offset + 13],
            data[slot_offset + 14],
            data[slot_offset + 15],
        ]);
        values.push(val);
    }
    values
}
