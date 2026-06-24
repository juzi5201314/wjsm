use super::*;
pub(crate) fn define_host_data_property_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    obj: i64,
    name: &str,
    val: i64,
) -> Option<()> {
    let name_id = find_memory_c_string_with_env(ctx, env, name)
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, name))?;
    define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        obj,
        encode_string_name_id(name_id),
        val,
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE,
    )
}

pub(crate) fn define_host_data_property_by_name_id_with_env<
    C: AsContextMut<Data = RuntimeState>,
>(
    ctx: &mut C,
    env: &WasmEnv,
    obj: i64,
    name_id: u32,
    val: i64,
    flags: i32,
) -> Option<()> {
    let obj_ptr = resolve_handle_idx_with_env(ctx, env, value::decode_object_handle(obj) as usize)?;
    let (capacity, num_props) = {
        let data = env.memory.data(&*ctx);
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
    if let Some((slot_offset, _, _)) =
        find_property_slot_by_name_id_with_env(ctx, env, obj_ptr, name_id)
    {
        let data = env.memory.data_mut(&mut *ctx);
        if slot_offset + 16 > data.len() {
            return None;
        }
        data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
        return Some(());
    }
    if num_props >= capacity {
        return None;
    }
    let data = env.memory.data_mut(&mut *ctx);
    let slot_offset = obj_ptr + 16 + num_props as usize * 32;
    if slot_offset + 32 > data.len() {
        return None;
    }
    let undef = value::encode_undefined();
    data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
    data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
    data[slot_offset + 16..slot_offset + 24].copy_from_slice(&undef.to_le_bytes());
    data[slot_offset + 24..slot_offset + 32].copy_from_slice(&undef.to_le_bytes());
    data[obj_ptr + 12..obj_ptr + 16].copy_from_slice(&(num_props + 1).to_le_bytes());
    Some(())
}

/// 定义一个访问器（getter/setter）属性到宿主创建的对象上（泛型版本，不支持 grow_object）。
/// slot 布局与数据属性相同（32字节），但 flags 标记为 IS_ACCESSOR，
/// offset 8 = undefined（保留），offset 16 = getter，offset 24 = setter。
pub(crate) fn define_host_accessor_property_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    obj: i64,
    name: &str,
    getter: i64,
    setter: i64,
) -> Option<()> {
    let name_id = find_memory_c_string_with_env(ctx, env, name)
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, name))?;
    let obj_ptr = resolve_handle_idx_with_env(ctx, env, value::decode_object_handle(obj) as usize)?;
    let (capacity, num_props) = {
        let data = env.memory.data(&*ctx);
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
    let actual_ptr = obj_ptr;
    let data = env.memory.data_mut(&mut *ctx);
    let slot_offset = actual_ptr + 16 + num_props as usize * 32;
    if slot_offset + 32 > data.len() {
        return None;
    }
    // 访问器属性：CONFIGURABLE | ENUMERABLE | IS_ACCESSOR（不含 WRITABLE）
    let flags =
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_IS_ACCESSOR;
    let undef = value::encode_undefined();
    data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
    data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    data[slot_offset + 8..slot_offset + 16].copy_from_slice(&undef.to_le_bytes());
    data[slot_offset + 16..slot_offset + 24].copy_from_slice(&getter.to_le_bytes());
    data[slot_offset + 24..slot_offset + 32].copy_from_slice(&setter.to_le_bytes());
    data[actual_ptr + 12..actual_ptr + 16].copy_from_slice(&(num_props + 1).to_le_bytes());
    Some(())
}

pub(crate) fn alloc_promise_all_settled_result(
    caller: &mut Caller<'_, RuntimeState>,
    status: &str,
    value_name: &str,
    value: i64,
) -> i64 {
    alloc_all_settled_result_from_caller(caller, status, value_name, value)
}

pub(crate) fn alloc_aggregate_error(caller: &mut Caller<'_, RuntimeState>, errors: i64) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    alloc_heap_aggregate_error(caller, &env, errors)
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
    if data[obj_ptr + 4] == wjsm_ir::HEAP_TYPE_ARRAY {
        let len = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]);
        let _ = data;
        let _ = mem;
        let mut names = Vec::new();
        for i in 0..len {
            if array_elem_present(caller, obj_ptr, i) {
                names.push(i.to_string());
            }
        }
        if !enumerable_only {
            names.push("length".to_string());
        }
        return names;
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
        if is_symbol_name_id(name_id) {
            continue;
        }
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
    if data[obj_ptr + 4] == wjsm_ir::HEAP_TYPE_ARRAY {
        let len = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]);
        let _ = data;
        let _ = mem;
        let mut values = Vec::new();
        for i in 0..len {
            if let Some(value) = read_array_elem(caller, obj_ptr, i) {
                values.push(value);
            }
        }
        if !enumerable_only {
            values.push(value::encode_f64(len as f64));
        }
        return values;
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
        let name_id = u32::from_le_bytes([
            data[slot_offset],
            data[slot_offset + 1],
            data[slot_offset + 2],
            data[slot_offset + 3],
        ]);
        if is_symbol_name_id(name_id) {
            continue;
        }
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
        values.push(value);
    }
    values
}

pub(crate) fn collect_own_property_key_values(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    symbols_only: bool,
) -> Vec<i64> {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return vec![];
    };
    let data = mem.data(&*caller);
    if obj_ptr + 16 > data.len() {
        return vec![];
    }
    if data[obj_ptr + 4] == wjsm_ir::HEAP_TYPE_ARRAY {
        if symbols_only {
            return vec![];
        }
        let len = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]);
        let _ = data;
        let _ = mem;
        let mut keys = Vec::new();
        for i in 0..len {
            if array_elem_present(caller, obj_ptr, i) {
                keys.push(store_runtime_string(caller, i.to_string()));
            }
        }
        keys.push(store_runtime_string(caller, "length".to_string()));
        return keys;
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
        name_ids.push(u32::from_le_bytes([
            data[slot_offset],
            data[slot_offset + 1],
            data[slot_offset + 2],
            data[slot_offset + 3],
        ]));
    }
    let _ = data;
    let _ = mem;

    let mut keys = Vec::new();
    for name_id in name_ids {
        if let Some(symbol_key) = name_id_to_property_key_value(name_id) {
            keys.push(symbol_key);
        } else if !symbols_only {
            let name_bytes = read_string_bytes(caller, name_id);
            keys.push(store_runtime_string(
                caller,
                String::from_utf8_lossy(&name_bytes).to_string(),
            ));
        }
    }
    keys
}

