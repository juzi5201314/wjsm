use super::*;

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
pub(crate) fn resolve_handle_idx(
    caller: &mut Caller<'_, RuntimeState>,
    handle_idx: usize,
) -> Option<usize> {
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
pub(crate) fn read_array_capacity(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
) -> Option<u32> {
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
pub(crate) fn read_array_elem(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
    index: u32,
) -> Option<i64> {
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
pub(crate) fn write_array_elem(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
    index: u32,
    val: i64,
) {
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
/// 对象动态扩容 — 遵循 capacity × 2 倍增策略，与 grow_array 同构
/// 对象槽位大小为 32 bytes（name_id:4 + flags:4 + value:8 + reserved:16）
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
pub(crate) fn merge_sort_by<T: Copy>(
    slice: &mut [T],
    cmp: &mut dyn FnMut(&T, &T) -> std::cmp::Ordering,
) {
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
pub(crate) fn enumerate_object_keys(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
) -> Vec<String> {
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
pub(crate) fn to_number(caller: &mut Caller<'_, RuntimeState>, val: i64) -> i64 {
    // undefined → NaN
    if value::is_undefined(val) {
        return f64::NAN.to_bits() as i64;
    }

    // null → +0
    if value::is_null(val) {
        return 0.0_f64.to_bits() as i64;
    }

    // bool: true → 1, false → 0
    if value::is_bool(val) {
        let b = value::decode_bool(val);
        return (if b { 1.0_f64 } else { 0.0_f64 }).to_bits() as i64;
    }

    // f64 → itself
    if value::is_f64(val) {
        return val;
    }

    // string → parseFloat (可能失败 → NaN)
    if value::is_string(val) {
        let s = if value::is_runtime_string_handle(val) {
            let handle = value::decode_runtime_string_handle(val) as usize;
            let strings = caller
                .data()
                .runtime_strings
                .lock()
                .expect("runtime strings mutex");
            strings.get(handle).cloned().unwrap_or_default()
        } else {
            read_string(caller, value::decode_string_ptr(val)).unwrap_or_default()
        };

        // 尝试解析字符串为数字
        // 先尝试 trim，然后解析
        let trimmed = s.trim();
        if let Ok(num) = trimmed.parse::<f64>() {
            return num.to_bits() as i64;
        }
        // 解析失败返回 NaN
        return f64::NAN.to_bits() as i64;
    }

    // BigInt → ToNumber: 转为 f64（可能丢失精度）
    if value::is_bigint(val) {
        let handle = value::decode_bigint_handle(val) as usize;
        let table = caller
            .data()
            .bigint_table
            .lock()
            .expect("bigint_table mutex");
        if let Some(bi) = table.get(handle) {
            if let Some(f) = bi.to_f64() {
                return f.to_bits() as i64;
            }
        }
        return f64::NAN.to_bits() as i64;
    }

    // RegExp → ToNumber: NaN (objects convert to NaN)
    if value::is_regexp(val) {
        return f64::NAN.to_bits() as i64;
    }

    // Symbol → ToNumber: 抛出 TypeError
    if value::is_symbol(val) {
        *caller
            .data()
            .runtime_error
            .lock()
            .expect("runtime error mutex") =
            Some("TypeError: Cannot convert a Symbol value to a number".to_string());
        return f64::NAN.to_bits() as i64;
    }

    // object/function → ToPrimitive(hint: Number) → ToNumber
    // 简化实现：调用 render_value 返回字符串，然后解析
    if value::is_object(val) || value::is_callable(val) {
        let prim = to_primitive(caller, val);
        return to_number(caller, prim);
    }

    // 其他类型（iterator, enumerator, exception）→ NaN
    f64::NAN.to_bits() as i64
}

/// ToPrimitive 抽象操作 (ECMAScript 7.1.1)
/// 将对象转换为原始值
/// 简化实现：调用 render_value 返回字符串
pub(crate) fn to_primitive(caller: &mut Caller<'_, RuntimeState>, val: i64) -> i64 {
    // 已经是原始类型
    if value::is_f64(val)
        || value::is_string(val)
        || value::is_bool(val)
        || value::is_undefined(val)
        || value::is_null(val)
        || value::is_bigint(val)
        || value::is_symbol(val)
    {
        return val;
    }

    // object/function → 调用 render_value 返回字符串表示
    if value::is_object(val) || value::is_callable(val) {
        if let Ok(s) = render_value(caller, val) {
            // 将字符串存入 runtime_strings
            let mut strings = caller
                .data()
                .runtime_strings
                .lock()
                .expect("runtime strings mutex");
            let handle = strings.len() as u32;
            strings.push(s);
            return value::encode_runtime_string_handle(handle);
        }
    }

    // 其他类型直接返回
    val
}

/// 严格相等比较 (ECMAScript 7.2.16)
pub(crate) fn strict_eq(caller: &mut Caller<'_, RuntimeState>, a: i64, b: i64) -> i64 {
    // 类型不同 → false
    let a_type = type_tag(a);
    let b_type = type_tag(b);

    if a_type != b_type {
        return value::encode_bool(false);
    }

    // 同类型比较
    match a_type {
        // f64: 注意 NaN !== NaN
        0 => {
            let af = f64::from_bits(a as u64);
            let bf = f64::from_bits(b as u64);
            if af.is_nan() || bf.is_nan() {
                return value::encode_bool(false);
            }
            value::encode_bool(af == bf)
        }
        // string
        1 => {
            let a_str = get_string_value(caller, a);
            let b_str = get_string_value(caller, b);
            value::encode_bool(a_str == b_str)
        }
        // undefined
        2 => value::encode_bool(true),
        // null
        3 => value::encode_bool(true),
        // bool
        4 => value::encode_bool(value::decode_bool(a) == value::decode_bool(b)),
        // BigInt: 值比较
        6 => {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let table = caller
                .data()
                .bigint_table
                .lock()
                .expect("bigint_table mutex");
            let eq = table
                .get(a_handle)
                .zip(table.get(b_handle))
                .map(|(x, y)| x == y)
                .unwrap_or(false);
            value::encode_bool(eq)
        }
        // Symbol: 引用比较（同一 handle）
        7 => value::encode_bool(a == b),
        // object/function/iterator/enumerator/exception: 引用比较
        _ => value::encode_bool(a == b),
    }
}

/// 获取类型标签 (用于 strict_eq)
/// 返回值: 0=f64, 1=string, 2=undefined, 3=null, 4=bool, 5=object/function/其他, 6=bigint, 7=symbol
pub(crate) fn type_tag(val: i64) -> u64 {
    if value::is_f64(val) {
        0
    } else if value::is_string(val) {
        1
    } else if value::is_undefined(val) {
        2
    } else if value::is_null(val) {
        3
    } else if value::is_bool(val) {
        4
    } else if value::is_bigint(val) {
        6
    } else if value::is_symbol(val) {
        7
    } else {
        5
    } // object, function, iterator, enumerator, exception, bound
}

/// 获取字符串值
pub(crate) fn get_string_value(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    if value::is_runtime_string_handle(val) {
        let handle = value::decode_runtime_string_handle(val) as usize;
        let strings = caller
            .data()
            .runtime_strings
            .lock()
            .expect("runtime strings mutex");
        strings.get(handle).cloned().unwrap_or_default()
    } else {
        read_string(caller, value::decode_string_ptr(val)).unwrap_or_default()
    }
}

/// 解析并调用函数（支持 TAG_FUNCTION / TAG_CLOSURE / TAG_BOUND）
pub(crate) fn resolve_and_call(
    caller: &mut Caller<'_, RuntimeState>,
    func: i64,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .unwrap();

    if value::is_bound(func) {
        let bound_idx = value::decode_bound_idx(func);
        let (target_func, bound_this, bound_args_ref) = {
            let bound = caller.data().bound_objects.lock().unwrap();
            let record = &bound[bound_idx as usize];
            (
                record.target_func,
                record.bound_this,
                record.bound_args.clone(),
            )
        };

        let total_count = bound_args_ref.len() as i32 + args_count;
        // 读取当前 shadow_sp
        let shadow_sp_global = caller
            .get_export("__shadow_sp")
            .and_then(|e| e.into_global())
            .unwrap();
        let shadow_sp = shadow_sp_global.get(&mut *caller).i32().unwrap();
        let ptr = shadow_sp;

        // Push bound_args at position 0
        for (i, arg) in bound_args_ref.iter().enumerate() {
            memory
                .write(
                    &mut *caller,
                    (ptr + i as i32 * 8) as usize,
                    &arg.to_le_bytes(),
                )
                .unwrap();
        }
        // Copy call args after
        for i in 0..args_count {
            let mut buf = [0u8; 8];
            memory
                .read(
                    &mut *caller,
                    (shadow_sp + args_base + i * 8) as usize,
                    &mut buf,
                )
                .unwrap();
            memory
                .write(
                    &mut *caller,
                    (ptr + (bound_args_ref.len() as i32 + i) * 8) as usize,
                    &buf,
                )
                .unwrap();
        }

        // 递归解析 target_func
        resolve_callable_and_call(caller, target_func, bound_this, ptr, total_count)
    } else {
        resolve_callable_and_call(caller, func, this_val, args_base, args_count)
    }
}

pub(crate) fn resolve_callable_and_call(
    caller: &mut Caller<'_, RuntimeState>,
    callee: i64,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (func_idx, env_obj) = if value::is_closure(callee) {
        let idx = value::decode_closure_idx(callee);
        let closures = caller.data().closures.lock().unwrap();
        let entry = &closures[idx as usize];
        (entry.func_idx, entry.env_obj)
    } else if value::is_function(callee) {
        (
            value::decode_function_idx(callee),
            value::encode_undefined(),
        )
    } else if value::is_bound(callee) {
        return resolve_and_call(caller, callee, this_val, args_base, args_count);
    } else if value::is_proxy(callee) {
        // Proxy apply trap: 查找 handler.apply，如果存在则调用
        let handle = value::decode_proxy_handle(callee) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().unwrap();
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                set_runtime_error(caller.data(), "TypeError: Cannot perform 'apply' on a proxy that has been revoked".to_string());
                return value::encode_undefined();
            }
            // 查找 apply trap
            if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                let trap = read_object_property_by_name(caller, handler_ptr, "apply")
                    .unwrap_or_else(value::encode_undefined);
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    // 收集 shadow stack 参数到数组
                    let args_arr = crate::runtime_host_helpers::alloc_array(caller, args_count as u32);
                    let memory = caller.get_export("memory").and_then(|e| e.into_memory()).unwrap();
                    for i in 0..args_count {
                        let mut buf = [0u8; 8];
                        let _ = memory.read(&mut *caller, (args_base + i * 8) as usize, &mut buf);
                        let arg_val = i64::from_le_bytes(buf);
                        crate::runtime_host_helpers::define_host_data_property(
                            caller, args_arr, &i.to_string(), arg_val,
                        );
                    }
                    return call_wasm_callback(caller, trap, entry.handler, &[entry.target, this_val, args_arr])
                        .unwrap_or_else(|_| value::encode_undefined());
                }
            }
            // 无 apply trap，转发到 target
            return resolve_callable_and_call(caller, entry.target, this_val, args_base, args_count);
        }
        return value::encode_undefined();
    } else {
        return value::encode_undefined();
    };

    let table = caller
        .get_export("__table")
        .and_then(|e| e.into_table())
        .unwrap();
    let func_ref = table.get(&mut *caller, func_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else {
        return value::encode_undefined();
    };
    let mut results = [Val::I64(0)];
    let _ = func.call(
        &mut *caller,
        &[
            Val::I64(env_obj),
            Val::I64(this_val),
            Val::I32(args_base),
            Val::I32(args_count),
        ],
        &mut results,
    );
    results[0].unwrap_i64()
}

pub(crate) fn func_apply_impl(
    caller: &mut Caller<'_, RuntimeState>,
    func: i64,
    this_val: i64,
    _args_array: i64,
) -> i64 {
    // args_array 是一个数组对象，需要展开其元素到 shadow stack
    // 简化实现：直接使用 func_call 语义但只支持固定参数
    // 完整实现需要读取数组元素
    resolve_and_call(caller, func, this_val, 0, 0)
}

pub(crate) fn func_bind_impl(
    caller: &mut Caller<'_, RuntimeState>,
    func: i64,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .unwrap();
    let mut bound_args = Vec::with_capacity(args_count as usize);
    for i in 0..args_count {
        let mut buf = [0u8; 8];
        memory
            .read(&mut *caller, (args_base + i * 8) as usize, &mut buf)
            .unwrap();
        bound_args.push(i64::from_le_bytes(buf));
    }
    let mut bound = caller.data().bound_objects.lock().unwrap();
    let idx = bound.len() as u32;
    bound.push(BoundRecord {
        target_func: func,
        bound_this: this_val,
        bound_args,
    });
    value::encode_bound_idx(idx)
}

pub(crate) fn object_rest_impl(
    _caller: &mut Caller<'_, RuntimeState>,
    _obj: i64,
    _excluded_keys: i64,
) -> i64 {
    // 简化实现：返回一个新的空对象
    // 完整实现需要遍历 obj 的属性并排除指定键
    value::encode_undefined()
}

pub(crate) fn obj_spread_impl(_caller: &mut Caller<'_, RuntimeState>, _dest: i64, _source: i64) {
    // 简化实现：不做任何复制
    // 完整实现需要遍历 source 的 own properties 并复制到 dest
}
