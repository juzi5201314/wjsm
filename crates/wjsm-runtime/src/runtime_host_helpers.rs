use super::*;

pub(crate) fn read_shadow_arg(
    caller: &mut Caller<'_, RuntimeState>,
    args_base: i32,
    index: u32,
) -> i64 {
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return value::encode_undefined();
    };
    let data = memory.data(&*caller);
    let offset = args_base as usize + (index as usize) * 8;
    if offset + 8 > data.len() {
        return value::encode_undefined();
    }
    i64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

// ── 辅助函数：调用 WASM 回调函数 ────────────────────────────────
pub(crate) fn call_wasm_callback(
    caller: &mut Caller<'_, RuntimeState>,
    func_val: i64,
    this_val: i64,
    args: &[i64],
) -> anyhow::Result<i64> {
    let shadow_sp_global = caller
        .get_export("__shadow_sp")
        .and_then(|e| e.into_global())
        .ok_or_else(|| anyhow::anyhow!("no __shadow_sp"))?;
    let shadow_sp = shadow_sp_global
        .get(&mut *caller)
        .i32()
        .ok_or_else(|| anyhow::anyhow!("shadow_sp not i32"))?;
    let new_shadow_sp = shadow_sp + (args.len() as i32) * 8;
    // 将参数写入影子栈
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return Err(anyhow::anyhow!("no memory"));
        };
        let data = memory.data_mut(&mut *caller);
        let mut write_pos = shadow_sp as usize;
        for &arg in args {
            if write_pos + 8 > data.len() {
                return Err(anyhow::anyhow!("shadow stack overflow"));
            }
            data[write_pos..write_pos + 8].copy_from_slice(&arg.to_le_bytes());
            write_pos += 8;
        }
    }
    // 更新 __shadow_sp
    shadow_sp_global.set(&mut *caller, Val::I32(new_shadow_sp))?;
    // 解析函数：支持闭包、函数引用、代理链
    let mut resolved = func_val;
    loop {
        if value::is_closure(resolved) || value::is_function(resolved) {
            break;
        }
        if !value::is_proxy(resolved) {
            return Err(anyhow::anyhow!("not callable"));
        }
        let handle = value::decode_proxy_handle(resolved) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().unwrap();
            table.get(handle).cloned()
        };
        let entry = match entry {
            Some(e) => e,
            None => return Err(anyhow::anyhow!("proxy handle not found")),
        };
        if entry.revoked {
            return Err(anyhow::anyhow!("proxy has been revoked"));
        }
        // Check handler.apply trap
        if let Some(handler_ptr) = resolve_handle(&mut *caller, entry.handler) {
            let trap = read_object_property_by_name(&mut *caller, handler_ptr, "apply")
                .unwrap_or_else(value::encode_undefined);
            if !value::is_undefined(trap) && !value::is_null(trap) {
                resolved = trap;
                continue;
            }
        }
        // No apply trap, forward to target
        resolved = entry.target;
        continue;
    }
    let (func_idx, env_obj) = if value::is_closure(resolved) {
        let idx = value::decode_closure_idx(resolved) as usize;
        let closures = caller.data().closures.lock().unwrap();
        if let Some(entry) = closures.get(idx) {
            (entry.func_idx, entry.env_obj)
        } else {
            return Err(anyhow::anyhow!("closure index out of range"));
        }
    } else if value::is_function(resolved) {
        (
            (resolved as u64 & 0xFFFF_FFFF) as u32,
            value::encode_undefined(),
        )
    } else {
        return Err(anyhow::anyhow!("not callable"));
    };
    // 通过函数表调用
    let table = caller
        .get_export("__table")
        .and_then(|e| e.into_table())
        .ok_or_else(|| anyhow::anyhow!("no __table"))?;
    let func_ref = table
        .get(&mut *caller, func_idx as u64)
        .ok_or_else(|| anyhow::anyhow!("table get failed"))?;
    let func = func_ref
        .as_func()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("table entry not a function"))?;
    let previous_new_target = caller.data().new_target.replace(value::encode_undefined());
    let mut results = [Val::I64(0)];
    let call_result = func.call(
        &mut *caller,
        &[
            Val::I64(env_obj),
            Val::I64(this_val),
            Val::I32(shadow_sp),
            Val::I32(args.len() as i32),
        ],
        &mut results,
    );
    // 恢复调用上下文（无论 call 成功与否）
    caller.data().new_target.set(previous_new_target);
    let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
    call_result?;
    Ok(results[0].unwrap_i64())
}

// ── 辅助函数：分配新数组 ────────────────────────────────────────
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
// ── arr_proto_push (#49) ──────────────────────────────────────────
/// 从 host 元素设置数组元素（直接写入堆内存）
pub(crate) fn set_array_elem(
    caller: &mut Caller<'_, RuntimeState>,
    arr_val: i64,
    index: i32,
    val: i64,
) {
    if !value::is_array(arr_val) {
        return;
    }
    let handle = value::decode_handle(arr_val) as usize;
    let Some(ptr) = resolve_handle_idx(caller, handle) else {
        return;
    };
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return;
    };
    let data = memory.data_mut(&mut *caller);
    let slot_offset = ptr + 16 + index as usize * 8;
    if slot_offset + 8 > data.len() {
        return;
    }
    data[slot_offset..slot_offset + 8].copy_from_slice(&val.to_le_bytes());
    // Update length to max(length, index+1)
    let old_len =
        u32::from_le_bytes([data[ptr + 8], data[ptr + 9], data[ptr + 10], data[ptr + 11]]);
    if (index as u32) >= old_len {
        let new_len = (index as u32) + 1;
        data[ptr + 8..ptr + 12].copy_from_slice(&new_len.to_le_bytes());
    }
}
// ── 辅助函数：分配新对象 ────────────────────────────────────────────
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
    d[ptr..ptr + 4].copy_from_slice(&0u32.to_le_bytes()); // proto = 0 (null)
    d[ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT;
    d[ptr + 5..ptr + 8].fill(0);
    d[ptr + 8..ptr + 12].copy_from_slice(&capacity.to_le_bytes()); // capacity
    d[ptr + 12..ptr + 16].copy_from_slice(&0u32.to_le_bytes()); // num_props = 0
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

pub(crate) fn alloc_promise(caller: &mut Caller<'_, RuntimeState>, entry: PromiseEntry) -> i64 {
    let promise = alloc_object(caller, 0);
    if value::is_object(promise) {
        let handle = value::decode_object_handle(promise) as usize;
        let mut table = caller
            .data()
            .promise_table
            .lock()
            .expect("promise table mutex");
        insert_promise_entry(&mut table, handle, entry);
    }
    promise
}

pub(crate) fn find_memory_c_string(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
) -> Option<u32> {
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

pub(crate) fn alloc_heap_c_string(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
) -> Option<u32> {
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
// ── 辅助函数：收集属性名/值 ──────────────────────────────────────────
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

pub(crate) fn value_to_number(arg: i64) -> f64 {
    if value::is_f64(arg) {
        value::decode_f64(arg)
    } else if value::is_bool(arg) {
        if value::decode_bool(arg) { 1.0 } else { 0.0 }
    } else if value::is_undefined(arg) {
        f64::NAN
    } else if value::is_null(arg) {
        0.0
    } else {
        f64::NAN
    }
}

pub(crate) fn is_callable_in_runtime(caller: &mut Caller<'_, RuntimeState>, val: i64) -> bool {
    if value::is_function(val) || value::is_closure(val) || value::is_bound(val) || value::is_native_callable(val) {
        return true;
    }
    if value::is_proxy(val) {
        let handle = value::decode_proxy_handle(val) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if !entry.revoked {
                return is_callable_in_runtime(caller, entry.target);
            }
        }
    }
    false
}

pub(crate) fn is_extensible_impl(caller: &mut Caller<'_, RuntimeState>, target: i64) -> bool {
    if !value::is_js_object(target) {
        return false;
    }
    if value::is_proxy(target) {
        let handle = value::decode_proxy_handle(target) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                return false;
            }
            return is_extensible_impl(caller, entry.target);
        }
        return false;
    }
    let set = caller.data().non_extensible_handles.lock().expect("non_extensible_handles mutex");
    !set.contains(&(target as u64))
}

pub(crate) fn prevent_extensions_impl(caller: &mut Caller<'_, RuntimeState>, target: i64) -> bool {
    if !value::is_js_object(target) {
        return false;
    }
    if value::is_proxy(target) {
        let handle = value::decode_proxy_handle(target) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                return false;
            }
            return prevent_extensions_impl(caller, entry.target);
        }
        return false;
    }
    let mut set = caller.data().non_extensible_handles.lock().expect("non_extensible_handles mutex");
    set.insert(target as u64);
    true
}

/// JS 属性描述符结构体，对应规范中 Property Descriptor 内部类型
#[derive(Debug, Clone)]
pub(crate) struct PropertyDescriptor {
    pub value: Option<i64>,
    pub writable: Option<bool>,
    pub enumerable: Option<bool>,
    pub configurable: Option<bool>,
    pub get: Option<i64>,
    pub set: Option<i64>,
}

/// 解析 JS 对象形式的描述符（desc）为 Rust 的 PropertyDescriptor 结构体
fn parse_descriptor(
    caller: &mut Caller<'_, RuntimeState>,
    desc_handle: i64,
) -> Result<PropertyDescriptor, String> {
    if !value::is_object(desc_handle)
        && !value::is_function(desc_handle)
        && !value::is_array(desc_handle)
        && !value::is_proxy(desc_handle)
    {
        return Err("TypeError: Invalid property descriptor".to_string());
    }
    let desc_ptr = match resolve_handle(caller, desc_handle) {
        Some(p) => p,
        None => return Err("TypeError: Invalid property descriptor".to_string()),
    };

    let prop_value = read_object_property_by_name(caller, desc_ptr, "value");
    let prop_writable = read_object_property_by_name(caller, desc_ptr, "writable");
    let prop_enumerable = read_object_property_by_name(caller, desc_ptr, "enumerable");
    let prop_configurable = read_object_property_by_name(caller, desc_ptr, "configurable");
    let prop_get = read_object_property_by_name(caller, desc_ptr, "get");
    let prop_set = read_object_property_by_name(caller, desc_ptr, "set");

    if let Some(getter) = prop_get {
        if !value::is_undefined(getter) && !value::is_null(getter) && !is_callable_in_runtime(caller, getter) {
            return Err("TypeError: property getter must be callable".to_string());
        }
    }
    if let Some(setter) = prop_set {
        if !value::is_undefined(setter) && !value::is_null(setter) && !is_callable_in_runtime(caller, setter) {
            return Err("TypeError: property setter must be callable".to_string());
        }
    }

    let has_accessor = (prop_get.is_some() && !value::is_undefined(prop_get.unwrap()))
        || (prop_set.is_some() && !value::is_undefined(prop_set.unwrap()));
    if has_accessor {
        if prop_value.is_some() {
            return Err("TypeError: Invalid property descriptor: cannot specify both accessor and value".to_string());
        }
        if prop_writable.is_some() {
            return Err("TypeError: Invalid property descriptor: cannot specify both accessor and writable".to_string());
        }
    }

    Ok(PropertyDescriptor {
        value: prop_value,
        writable: prop_writable.map(|v| !value::is_falsy(v)),
        enumerable: prop_enumerable.map(|v| !value::is_falsy(v)),
        configurable: prop_configurable.map(|v| !value::is_falsy(v)),
        get: prop_get,
        set: prop_set,
    })
}

/// 从 target 对象的指定属性中提取出 PropertyDescriptor 结构体
fn get_target_descriptor(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: u32,
) -> Option<PropertyDescriptor> {
    let obj_ptr = resolve_handle(caller, target)?;
    let (slot_offset, flags, val) = find_property_slot_by_name_id(caller, obj_ptr, name_id)?;

    let is_accessor = (flags & constants::FLAG_IS_ACCESSOR) != 0;
    let configurable = (flags & constants::FLAG_CONFIGURABLE) != 0;
    let enumerable = (flags & constants::FLAG_ENUMERABLE) != 0;
    let writable = (flags & constants::FLAG_WRITABLE) != 0;

    let (getter, setter) = if is_accessor {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(caller);
        let getter = i64::from_le_bytes(data[slot_offset + 16..slot_offset + 24].try_into().unwrap());
        let setter = i64::from_le_bytes(data[slot_offset + 24..slot_offset + 32].try_into().unwrap());
        (Some(getter), Some(setter))
    } else {
        (None, None)
    };

    Some(PropertyDescriptor {
        value: if !is_accessor { Some(val) } else { None },
        writable: if !is_accessor { Some(writable) } else { None },
        enumerable: Some(enumerable),
        configurable: Some(configurable),
        get: getter,
        set: setter,
    })
}

/// 导出供 Object.defineProperty 及 Reflect.defineProperty 共享调用的核心底层属性定义逻辑
pub(crate) fn define_property_internal(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
    descriptor: i64,
) -> Result<bool, String> {
    if !value::is_object(target)
        && !value::is_function(target)
        && !value::is_array(target)
        && !value::is_proxy(target)
    {
        return Err("TypeError: Object.defineProperty called on non-object".to_string());
    }

    let desc = parse_descriptor(caller, descriptor)?;

    let prop_name = match render_value(caller, prop) {
        Ok(s) => s,
        Err(_) => return Err("TypeError: Cannot convert property key to string".to_string()),
    };
    let name_id = match find_memory_c_string(caller, &prop_name)
        .or_else(|| alloc_heap_c_string(caller, &prop_name))
    {
        Some(id) => id,
        None => return Err("TypeError: Failed to allocate property name string".to_string()),
    };

    define_property_on_target(caller, target, name_id, prop, descriptor, &desc)
}

/// 递归分发，同时处理普通对象与 Proxy 对象的属性定义与 invariants 校验
fn define_property_on_target(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: u32,
    prop_val: i64,
    descriptor: i64,
    desc: &PropertyDescriptor,
) -> Result<bool, String> {
    if value::is_proxy(target) {
        let handle = value::decode_proxy_handle(target) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        let entry = match entry {
            Some(e) => e,
            None => return Err("TypeError: Proxy target not found".to_string()),
        };
        if entry.revoked {
            return Err("TypeError: Cannot perform 'defineProperty' on a proxy that has been revoked".to_string());
        }

        if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
            let trap = read_object_property_by_name(caller, handler_ptr, "defineProperty")
                .unwrap_or_else(value::encode_undefined);
            if !value::is_undefined(trap) && !value::is_null(trap) {
                // 触发 defineProperty trap: (target, property, descriptor)
                let result = match call_wasm_callback(
                    caller,
                    trap,
                    entry.handler,
                    &[entry.target, prop_val, descriptor],
                ) {
                    Ok(res) => res,
                    Err(e) => return Err(format!("TypeError: defineProperty trap failed: {}", e)),
                };
                let trap_result = !value::is_falsy(result);
                if !trap_result {
                    return Ok(false);
                }

                // Invariants 检查（ECMAScript 规范代理定义属性的不变式）
                let target_desc = get_target_descriptor(caller, entry.target, name_id);
                let extensible = is_extensible_impl(caller, entry.target);
                let setting_config_false = desc.configurable == Some(false);

                if target_desc.is_none() {
                    if !extensible {
                        return Err("TypeError: proxy defineProperty invariant violation: target is non-extensible and property does not exist".to_string());
                    }
                    if setting_config_false {
                        return Err("TypeError: proxy defineProperty invariant violation: cannot define non-configurable property if it does not exist on target".to_string());
                    }
                } else {
                    let td = target_desc.unwrap();
                    let target_configurable = td.configurable.unwrap_or(true);

                    if !target_configurable {
                        if desc.configurable == Some(true) {
                            return Err("TypeError: proxy defineProperty invariant violation: cannot redefine non-configurable property as configurable".to_string());
                        }
                        if let Some(new_enum) = desc.enumerable {
                            if Some(new_enum) != td.enumerable {
                                return Err("TypeError: proxy defineProperty invariant violation: cannot change enumerableness of non-configurable property".to_string());
                            }
                        }

                        let is_new_accessor = desc.get.is_some() || desc.set.is_some();
                        let target_accessor = td.get.is_some() || td.set.is_some();
                        if is_new_accessor != target_accessor {
                            return Err("TypeError: proxy defineProperty invariant violation: cannot change property type on non-configurable property".to_string());
                        }

                        if !target_accessor {
                            // 数据属性
                            let target_writable = td.writable.unwrap_or(true);
                            if !target_writable {
                                if desc.writable == Some(true) {
                                    return Err("TypeError: proxy defineProperty invariant violation: cannot make non-writable property writable".to_string());
                                }
                                if let Some(new_val) = desc.value {
                                    let old_val = td.value.unwrap_or(value::encode_undefined());
                                    let same = strict_eq(caller, old_val, new_val);
                                    if value::is_falsy(same) {
                                        return Err("TypeError: proxy defineProperty invariant violation: cannot change value of non-writable non-configurable property".to_string());
                                    }
                                }
                            }
                        } else {
                            // 访问器属性
                            if let Some(new_getter) = desc.get {
                                let old_getter = td.get.unwrap_or(value::encode_undefined());
                                if new_getter != old_getter {
                                    return Err("TypeError: proxy defineProperty invariant violation: cannot change getter of non-configurable property".to_string());
                                }
                            }
                            if let Some(new_setter) = desc.set {
                                let old_setter = td.set.unwrap_or(value::encode_undefined());
                                if new_setter != old_setter {
                                    return Err("TypeError: proxy defineProperty invariant violation: cannot change setter of non-configurable property".to_string());
                                }
                            }
                        }
                    }
                }

                // 规范要求：在通过 invariants 后，需对底层 target 实际完成定义写入以同步影子状态
                return define_property_on_target(
                    caller,
                    entry.target,
                    name_id,
                    prop_val,
                    descriptor,
                    desc,
                );
            }
        }

        // 无 trap，转发到被代理的 target
        define_property_on_target(caller, entry.target, name_id, prop_val, descriptor, desc)
    } else {
        // 普通对象属性定义
        define_property_on_normal_object(caller, target, name_id, desc)
    }
}

/// 在底层普通（非代理）对象上实际执行属性定义（包含 invariants 检查与现有属性重写）
fn define_property_on_normal_object(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: u32,
    desc: &PropertyDescriptor,
) -> Result<bool, String> {
    let obj_ptr = match resolve_handle(caller, target) {
        Some(p) => p,
        None => return Err("TypeError: Invalid target object".to_string()),
    };

    let found = find_property_slot_by_name_id(caller, obj_ptr, name_id);
    if let Some((slot_offset, old_flags, old_val)) = found {
        // 属性已存在
        let old_configurable = (old_flags & constants::FLAG_CONFIGURABLE) != 0;
        let old_enumerable = (old_flags & constants::FLAG_ENUMERABLE) != 0;
        let old_writable = (old_flags & constants::FLAG_WRITABLE) != 0;
        let old_accessor = (old_flags & constants::FLAG_IS_ACCESSOR) != 0;

        // 提前安全地读取 old_getter 与 old_setter，避免在闭包中捕获或多次移动 caller
        let (old_getter, old_setter) = if old_accessor {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return Err("TypeError: Memory not found".to_string());
            };
            let data = memory.data(&*caller);
            let g = i64::from_le_bytes(data[slot_offset + 16..slot_offset + 24].try_into().unwrap());
            let s = i64::from_le_bytes(data[slot_offset + 24..slot_offset + 32].try_into().unwrap());
            (g, s)
        } else {
            (value::encode_undefined(), value::encode_undefined())
        };

        // 如果不可配置属性，执行严格的 invariants 检查
        if !old_configurable {
            if desc.configurable == Some(true) {
                return Err("TypeError: Cannot redefine non-configurable property".to_string());
            }
            if let Some(new_enum) = desc.enumerable {
                if new_enum != old_enumerable {
                    return Err("TypeError: Cannot redefine enumerable attribute of non-configurable property".to_string());
                }
            }
            let is_new_accessor = desc.get.is_some() || desc.set.is_some();
            if is_new_accessor != old_accessor {
                return Err("TypeError: Cannot change property type from data to accessor or vice versa on non-configurable property".to_string());
            }
            if !old_accessor {
                // 数据属性
                if !old_writable {
                    if desc.writable == Some(true) {
                        return Err("TypeError: Cannot make non-writable property writable".to_string());
                    }
                    if let Some(new_val) = desc.value {
                        let same = strict_eq(caller, old_val, new_val);
                        if value::is_falsy(same) {
                            return Err("TypeError: Cannot change value of non-configurable non-writable property".to_string());
                        }
                    }
                }
            } else {
                // 访问器属性
                if let Some(new_getter) = desc.get {
                    if new_getter != old_getter {
                        return Err("TypeError: Cannot change getter of non-configurable property".to_string());
                    }
                }
                if let Some(new_setter) = desc.set {
                    if new_setter != old_setter {
                        return Err("TypeError: Cannot change setter of non-configurable property".to_string());
                    }
                }
            }
        }

        // 计算新 flags
        let is_accessor = desc.get.is_some()
            || desc.set.is_some()
            || (desc.value.is_none() && desc.writable.is_none() && old_accessor);
        let mut flags: i32 = 0;
        if is_accessor {
            flags |= constants::FLAG_IS_ACCESSOR;
        }

        let writable = desc
            .writable
            .unwrap_or(if !is_accessor { old_writable } else { false });
        if writable {
            flags |= constants::FLAG_WRITABLE;
        }
        let enumerable = desc.enumerable.unwrap_or(old_enumerable);
        if enumerable {
            flags |= constants::FLAG_ENUMERABLE;
        }
        let configurable = desc.configurable.unwrap_or(old_configurable);
        if configurable {
            flags |= constants::FLAG_CONFIGURABLE;
        }

        let val = desc.value.unwrap_or(old_val);
        let getter = desc.get.unwrap_or(old_getter);
        let setter = desc.set.unwrap_or(old_setter);

        // 写入已有 slot 覆盖
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return Ok(false);
        };
        let data = memory.data_mut(&mut *caller);
        data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
        data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
        data[slot_offset + 16..slot_offset + 24].copy_from_slice(&getter.to_le_bytes());
        data[slot_offset + 24..slot_offset + 32].copy_from_slice(&setter.to_le_bytes());

        Ok(true)
    } else {
        // 新增属性
        if !is_extensible_impl(caller, target) {
            return Err("TypeError: Cannot add property to non-extensible object".to_string());
        }

        let is_accessor = desc.get.is_some() || desc.set.is_some();
        let mut flags: i32 = 0;
        if is_accessor {
            flags |= constants::FLAG_IS_ACCESSOR;
        }
        if desc.writable.unwrap_or(false) && !is_accessor {
            flags |= constants::FLAG_WRITABLE;
        }
        if desc.enumerable.unwrap_or(false) {
            flags |= constants::FLAG_ENUMERABLE;
        }
        if desc.configurable.unwrap_or(false) {
            flags |= constants::FLAG_CONFIGURABLE;
        }

        let val = desc.value.unwrap_or(value::encode_undefined());
        let getter = desc.get.unwrap_or(value::encode_undefined());
        let setter = desc.set.unwrap_or(value::encode_undefined());

        write_new_property_to_memory(caller, target, name_id, flags, val, getter, setter);
        Ok(true)
    }
}

/// 移植的低级对象属性扩容与新增写入逻辑
pub(crate) fn write_new_property_to_memory(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: u32,
    flags: i32,
    val: i64,
    getter: i64,
    setter: i64,
) {
    let obj_ptr = match resolve_handle(caller, target) {
        Some(p) => p,
        None => return,
    };

    let (capacity, num_props) = {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return;
        };
        let data = memory.data(&*caller);
        if obj_ptr + 16 > data.len() {
            return;
        }
        let capacity = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]) as usize;
        let num_props = u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]) as usize;
        (capacity, num_props)
    };

    let mut actual_obj_ptr = obj_ptr;

    if num_props >= capacity {
        let obj_table_ptr = {
            let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else {
                return;
            };
            g.get(&mut *caller).i32().unwrap_or(0) as usize
        };
        let heap_ptr = {
            let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
                return;
            };
            g.get(&mut *caller).i32().unwrap_or(0) as usize
        };
        let handle_idx = (target as u64 & 0xFFFF_FFFF) as u32;

        let new_capacity = if capacity == 0 { 1 } else { capacity * 2 };
        let new_size = 16 + new_capacity * 32;

        {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return;
            };
            let data = memory.data_mut(&mut *caller);
            if heap_ptr + new_size > data.len() {
                return;
            }

            let old_size = 16 + num_props * 32;
            data.copy_within(actual_obj_ptr..actual_obj_ptr + old_size, heap_ptr);

            data[heap_ptr + 8..heap_ptr + 12]
                .copy_from_slice(&(new_capacity as u32).to_le_bytes());

            let slot_addr = obj_table_ptr + handle_idx as usize * 4;
            if slot_addr + 4 <= data.len() {
                data[slot_addr..slot_addr + 4]
                    .copy_from_slice(&(heap_ptr as u32).to_le_bytes());
            }
        }

        {
            let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
                return;
            };
            let _ = g.set(&mut *caller, Val::I32((heap_ptr + new_size) as i32));
        }

        actual_obj_ptr = heap_ptr;
    }

    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return;
    };
    let data = memory.data_mut(&mut *caller);
    let slot_offset = actual_obj_ptr + 16 + num_props * 32;
    if slot_offset + 32 > data.len() {
        return;
    }
    data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
    data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
    data[slot_offset + 16..slot_offset + 24].copy_from_slice(&getter.to_le_bytes());
    data[slot_offset + 24..slot_offset + 32].copy_from_slice(&setter.to_le_bytes());
    let new_num_props = num_props + 1;
    data[actual_obj_ptr + 12..actual_obj_ptr + 16]
        .copy_from_slice(&(new_num_props as u32).to_le_bytes());
}

pub(crate) fn reflect_get_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
) -> i64 {
    if value::is_proxy(target) {
        let handle = value::decode_proxy_handle(target) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                set_runtime_error(caller.data(), "TypeError: Cannot perform 'get' on a proxy that has been revoked".to_string());
                return value::encode_undefined();
            }
            if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                let trap = read_object_property_by_name(caller, handler_ptr, "get")
                    .unwrap_or_else(value::encode_undefined);
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    return call_wasm_callback(caller, trap, entry.handler, &[entry.target, prop, target])
                        .unwrap_or_else(|_| value::encode_undefined());
                }
            }
            // 无 trap，转发到 target（递归）
            return reflect_get_impl(caller, entry.target, prop);
        }
        return value::encode_undefined();
    }

    // 非 Proxy 对象
    let obj_ptr = resolve_handle(caller, target);
    let prop_name = render_value(caller, prop).ok();
    let existing_val = obj_ptr.and_then(|ptr| {
        prop_name.as_ref()
            .and_then(|name| read_object_property_by_name(caller, ptr, name))
    });

    let is_proto_req = prop_name.as_deref() == Some("prototype");

    if is_proto_req && (value::is_function(target) || value::is_closure(target) || value::is_bound(target)) {
        match existing_val {
            Some(v) if !value::is_undefined(v) => return v,
            _ => {
                // 创建默认 prototype 对象并作为函数的 own property 写入，GC 可自然追踪
                let default_proto = alloc_host_object_from_caller(caller, 4);
                let _ = define_host_data_property_from_caller(caller, target, "prototype", default_proto);
                // 设置 proto 的 constructor 属性
                let ctor_prop_name_id = find_memory_c_string(caller, "constructor");
                if let Some(name_c) = ctor_prop_name_id {
                    if let Some(dp_ptr) = resolve_handle(caller, default_proto) {
                        write_object_property_by_name_id(
                            caller,
                            dp_ptr,
                            default_proto,
                            name_c as u32,
                            target,
                            constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
                        );
                    }
                }
                return default_proto;
            }
        }
    }

    if let Some(v) = existing_val {
        return v;
    }
    value::encode_undefined()
}


