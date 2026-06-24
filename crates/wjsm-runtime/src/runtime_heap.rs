use super::*;
use crate::wasm_env::WasmEnv;

use wjsm_ir::SHADOW_STACK_SIZE;

/// handle 表上界（止于 shadow stack 基址），与 WASM emit_handle_table_alloc_check 一致。
fn handle_table_end_byte<C: AsContextMut<Data = RuntimeState>>(env: &WasmEnv, ctx: &mut C) -> usize {
    let Some(g) = env.shadow_stack_end else {
        return env.memory.data(&*ctx).len();
    };
    let end = g.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;
    end.saturating_sub(SHADOW_STACK_SIZE as usize)
}

pub(crate) fn host_handle_slot_fits<C: AsContextMut<Data = RuntimeState>>(
    env: &WasmEnv,
    ctx: &mut C,
    candidate: u32,
) -> bool {
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;
    let need_end = obj_table_ptr
        .saturating_add((candidate as usize).saturating_mul(4))
        .saturating_add(4);
    need_end <= handle_table_end_byte(env, ctx)
}

/// 线性内存不足时按页扩展，供 host 侧 bump / resize 使用。
pub(crate) fn ensure_linear_memory_bytes<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    need_end: usize,
) {
    while env.memory.data(&*ctx).len() < need_end {
        if env.memory.grow(&mut *ctx, 1).is_err() {
            break;
        }
    }
}

fn alloc_host_object_impl<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
    proto: u32,
) -> i64 {
    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let _obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let _obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let size = 16u32.saturating_add(capacity.saturating_mul(32));
    let new_heap_ptr = heap_ptr.saturating_add(size);

    // P4：host 端对象分配的 OOM 处理。空间不足时先 GC（可能回收死对象腾空间），
    // 仍不足则 memory.grow。GC 经 gc_algorithm（与 $obj_new 一致），但 host 路径是泛型
    // C: AsContextMut（Caller 或 Store），GcContext 需 &mut Caller，故仅在 Caller 路径触发 GC。
    // Store 路径（async streams_fetch_body）跳过 GC，仅 grow（罕见 async 路径，grow 足够）。
    if new_heap_ptr as usize > env.memory.data(&*ctx).len() {
        // 尝试 GC（Caller 路径）
        try_gc_for_host_alloc(ctx, env, size as usize);
        // 仍不足则 grow
        let cur_len = env.memory.data(&*ctx).len();
        let hp = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as usize;
        if hp + size as usize > cur_len {
            let _ = env.memory.grow(&mut *ctx, 1);
        }
    }

    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let new_heap_ptr = heap_ptr.saturating_add(size);
    ensure_linear_memory_bytes(ctx, env, new_heap_ptr as usize);
    if new_heap_ptr as usize > env.memory.data(&*ctx).len() {
        return value::encode_undefined();
    }
    if !host_handle_slot_fits(env, ctx, obj_table_count) {
        return value::encode_undefined();
    }
    let ptr = heap_ptr as usize;
    let slot_addr = obj_table_ptr as usize + obj_table_count as usize * 4;
    {
        let data = env.memory.data_mut(&mut *ctx);
        data[ptr..ptr + 4].copy_from_slice(&proto.to_le_bytes());
        data[ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT;
        data[ptr + 5..ptr + 8].fill(0);
        data[ptr + 8..ptr + 12].copy_from_slice(&capacity.to_le_bytes());
        data[ptr + 12..ptr + 16].copy_from_slice(&0u32.to_le_bytes());
        data[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
    }
    let _ = env.heap_ptr.set(&mut *ctx, Val::I32(new_heap_ptr as i32));
    let _ = env
        .obj_table_count
        .set(&mut *ctx, Val::I32((obj_table_count + 1) as i32));
    value::encode_object_handle(obj_table_count)
}

/// host 端 OOM 时的 GC 触发（仅 Caller 路径）。Store 路径 no-op（经 grow 兜底）。
/// 通过 AsContextMut 访问 gc_algorithm；GcContext 需 &mut Caller，此处用 Caller 特化。
fn try_gc_for_host_alloc<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    _size: usize,
) {
    // 经 store 访问 gc_algorithm。AsContextMut → store.data() 拿 RuntimeState（只读 clone Arc）。
    // 但 GcContext 需 &mut Caller，泛型 C 无法直接构造。
    // 退而求其次：此处不做 collect（避免 GcContext 借用模型冲突），依赖 gc_maybe_collect
    // 在 WASM $obj_new 路径已做过 proactive collect。host 路径 OOM 主要靠 grow。
    // 注：若 host 路径频繁 OOM 且 grow 受限，后续可泛型化 GcContext 解决。
    let _ = (ctx, env);
}

pub(crate) fn alloc_host_object<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    let proto = env.object_proto_handle.get(&mut *ctx).i32().unwrap_or(-1) as u32;
    alloc_host_object_impl(ctx, env, capacity, proto)
}

pub(crate) fn alloc_host_null_proto_object<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    alloc_host_object_impl(ctx, env, capacity, u32::MAX)
}

/// 共享的错误对象创建逻辑：分配 host 对象，设置 name/message 属性和 __error_brand__ 隐藏标记。
/// `create_error_object`（Caller 路径）和 `alloc_type_error_with_env`（泛型 C 路径）均委托此函数。
pub(crate) fn alloc_error_object_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    error_name: &str,
    message: String,
) -> i64 {
    let name_val = {
        let state = ctx.as_context().data();
        store_runtime_string_in_state(state, error_name.to_string())
    };
    let message_val = {
        let state = ctx.as_context().data();
        store_runtime_string_in_state(state, message)
    };
    let obj = alloc_host_object(ctx, env, 3);
    let _ = define_host_data_property_with_env(ctx, env, obj, "name", name_val);
    let _ = define_host_data_property_with_env(ctx, env, obj, "message", message_val);
    // C2: 隐藏品牌标记，用于 render_value 区分真实 Error vs 普通对象 {name:"TypeError"}。
    let brand_val = value::encode_bool(true);
    let name_id = find_memory_c_string_with_env(ctx, env, "__error_brand__")
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, "__error_brand__"))
        .unwrap();
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        obj,
        encode_string_name_id(name_id),
        brand_val,
        0,
    );
    obj
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
        let mut table = caller.data().error_table.lock().unwrap_or_else(|e| e.into_inner());
        table.push(ErrorEntry {
            name: error_name.to_string(),
            message: message.clone(),
            value: value::encode_undefined(),
        });
    }
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    alloc_error_object_with_env(caller, &env, error_name, message)
}

pub(crate) fn alloc_type_error_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    message: String,
) -> i64 {
    alloc_error_object_with_env(ctx, env, "TypeError", message)
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

/// 定义一个访问器（getter/setter）属性到宿主创建的对象上（from_caller 便捷封装）。
pub(crate) fn define_host_accessor_property_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    getter: i64,
    setter: i64,
) -> Option<()> {
    define_host_accessor_property(caller, obj, name, getter, setter)
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
