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
    // 解析函数（闭包或函数引用）
    let (func_idx, env_obj) = if value::is_closure(func_val) {
        let idx = value::decode_closure_idx(func_val) as usize;
        let closures = caller.data().closures.lock().unwrap();
        if let Some(entry) = closures.get(idx) {
            (entry.func_idx, entry.env_obj)
        } else {
            return Err(anyhow::anyhow!("closure index out of range"));
        }
    } else if value::is_function(func_val) {
        (
            (func_val as u64 & 0xFFFF_FFFF) as u32,
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
    // 恢复 __shadow_sp（无论 call 成功与否）
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
