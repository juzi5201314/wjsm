use super::*;
use crate::wasm_env::WasmEnv;

pub(crate) fn alloc_host_object<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let size = 16 + capacity * 32;
    let new_heap_ptr = heap_ptr.saturating_add(size);
    let proto = env.object_proto_handle.get(&mut *ctx).i32().unwrap_or(-1) as u32;
    {
        let data = env.memory.data_mut(&mut *ctx);
        let ptr = heap_ptr as usize;
        if new_heap_ptr as usize > data.len() {
            return value::encode_undefined();
        }
        data[ptr..ptr + 4].copy_from_slice(&proto.to_le_bytes());
        data[ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT;
        data[ptr + 5..ptr + 8].fill(0);
        data[ptr + 8..ptr + 12].copy_from_slice(&capacity.to_le_bytes());
        data[ptr + 12..ptr + 16].copy_from_slice(&0u32.to_le_bytes());
        let slot_addr = (obj_table_ptr + obj_table_count * 4) as usize;
        if slot_addr + 4 <= data.len() {
            data[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
        }
    }
    let _ = env.heap_ptr.set(&mut *ctx, Val::I32(new_heap_ptr as i32));
    let _ = env
        .obj_table_count
        .set(&mut *ctx, Val::I32((obj_table_count + 1) as i32));
    value::encode_object_handle(obj_table_count)
}

pub(crate) fn alloc_host_null_proto_object<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let size = 16 + capacity * 32;
    let new_heap_ptr = heap_ptr.saturating_add(size);
    {
        let data = env.memory.data_mut(&mut *ctx);
        let ptr = heap_ptr as usize;
        if new_heap_ptr as usize > data.len() {
            return value::encode_undefined();
        }
        data[ptr..ptr + 4].copy_from_slice(&(-1i32 as u32).to_le_bytes());
        data[ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT;
        data[ptr + 5..ptr + 8].fill(0);
        data[ptr + 8..ptr + 12].copy_from_slice(&capacity.to_le_bytes());
        data[ptr + 12..ptr + 16].copy_from_slice(&0u32.to_le_bytes());
        let slot_addr = (obj_table_ptr + obj_table_count * 4) as usize;
        if slot_addr + 4 <= data.len() {
            data[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
        }
    }
    let _ = env.heap_ptr.set(&mut *ctx, Val::I32(new_heap_ptr as i32));
    let _ = env
        .obj_table_count
        .set(&mut *ctx, Val::I32((obj_table_count + 1) as i32));
    value::encode_object_handle(obj_table_count)
}

pub(crate) fn create_error_object(
    caller: &mut Caller<'_, RuntimeState>,
    error_name: &str,
    arg: i64,
) -> i64 {
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
        if value::decode_bool(arg) {
            "true".to_string()
        } else {
            "false".to_string()
        }
    } else {
        String::new()
    };
    {
        let mut table = caller.data().error_table.lock().expect("error table mutex");
        table.push(ErrorEntry {
            name: error_name.to_string(),
            message: message.clone(),
            value: value::encode_undefined(),
        });
    }
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
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
        let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(obj) as usize);
        if let Some(op) = obj_ptr {
            if let Some(Extern::Memory(mem)) = caller.get_export("memory") {
                let data = mem.data(&caller);
                if op + 4 < data.len() && data[op + 4] == wjsm_ir::HEAP_TYPE_ARGUMENTS {
                    return store_runtime_string(caller, "[object Arguments]".to_string());
                }
            }
            let map_handle = read_object_property_by_name(caller, op, "__map_handle__");
            if map_handle.is_some() {
                return store_runtime_string(caller, "[object Map]".to_string());
            }
            let set_handle = read_object_property_by_name(caller, op, "__set_handle__");
            if set_handle.is_some() {
                return store_runtime_string(caller, "[object Set]".to_string());
            }
        }
        let name_val = obj_ptr.and_then(|p| read_object_property_by_name(caller, p, "name"));
        let msg_val = obj_ptr.and_then(|p| read_object_property_by_name(caller, p, "message"));
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
                    let obj_ptr2 =
                        resolve_handle_idx(caller, value::decode_object_handle(obj) as usize);
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
    define_host_data_property(caller, obj, name, val)
}

pub(crate) fn alloc_all_settled_result_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    status: &str,
    value_name: &str,
    val: i64,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let status_value = store_runtime_string(caller, status.to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "status", status_value);
    let _ = define_host_data_property_from_caller(caller, obj, value_name, val);
    obj
}

pub(crate) fn alloc_aggregate_error_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    errors: i64,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 3);
    let name = store_runtime_string(caller, "AggregateError".to_string());
    let message = store_runtime_string(caller, "All promises were rejected".to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "name", name);
    let _ = define_host_data_property_from_caller(caller, obj, "message", message);
    let _ = define_host_data_property_from_caller(caller, obj, "errors", errors);
    obj
}

pub(crate) fn alloc_all_settled_result<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    status: &str,
    value_name: &str,
    val: i64,
) -> i64 {
    let obj = alloc_host_object(ctx, env, 2);
    let status_value =
        store_runtime_string_in_state(ctx.as_context_mut().data_mut(), status.to_string());
    let _ = define_host_data_property_with_env(ctx, env, obj, "status", status_value);
    let _ = define_host_data_property_with_env(ctx, env, obj, value_name, val);
    obj
}

pub(crate) fn alloc_heap_aggregate_error<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    errors: i64,
) -> i64 {
    let obj = alloc_host_object(ctx, env, 3);
    let name = store_runtime_string_in_state(
        ctx.as_context_mut().data_mut(),
        "AggregateError".to_string(),
    );
    let message = store_runtime_string_in_state(
        ctx.as_context_mut().data_mut(),
        "All promises were rejected".to_string(),
    );
    let _ = define_host_data_property_with_env(ctx, env, obj, "name", name);
    let _ = define_host_data_property_with_env(ctx, env, obj, "message", message);
    let _ = define_host_data_property_with_env(ctx, env, obj, "errors", errors);
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
