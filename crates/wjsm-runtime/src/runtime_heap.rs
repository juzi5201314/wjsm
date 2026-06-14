use super::*;
use crate::wasm_env::WasmEnv;

fn alloc_host_object_impl<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
    proto: u32,
) -> i64 {
    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
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
    {
        let data = env.memory.data_mut(&mut *ctx);
        let ptr = heap_ptr as usize;
        if new_heap_ptr as usize > data.len() {
            return value::encode_undefined();
        }
        let slot_addr = obj_table_ptr as usize + obj_table_count as usize * 4;
        // obj_table 槽位耗尽时必须直接返回 undefined，不递增 obj_table_count、
        // 不前进 heap_ptr，保持 handle->slot 映射与 obj_table_count 一致。
        if slot_addr + 4 > data.len() {
            return value::encode_undefined();
        }
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
    let obj = alloc_host_object(caller, &env, 3);
    let name_val = store_runtime_string(caller, error_name.to_string());
    let _ = define_host_data_property(caller, obj, "name", name_val);
    let message_val = store_runtime_string(caller, message);
    let _ = define_host_data_property(caller, obj, "message", message_val);
    // C2: 隐藏品牌标记，用于 render_value 区分真实 Error vs 普通对象 {name:"TypeError"}。
    let brand_val = value::encode_bool(true);
    let env2 = WasmEnv::from_caller(caller).expect("WasmEnv");
    let name_id = find_memory_c_string_with_env(caller, &env2, "__error_brand__")
        .or_else(|| alloc_heap_c_string_with_env(caller, &env2, "__error_brand__"))
        .unwrap();
    let _ = define_host_data_property_by_name_id_with_flags(
        caller,
        obj,
        encode_string_name_id(name_id),
        brand_val,
        0, // 非枚举
    );
    obj
}

pub(crate) fn alloc_type_error_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    message: String,
) -> i64 {
    let name_val = {
        let state = ctx.as_context().data();
        store_runtime_string_in_state(state, "TypeError".to_string())
    };
    let message_val = {
        let state = ctx.as_context().data();
        store_runtime_string_in_state(state, message)
    };
    let obj = alloc_host_object(ctx, env, 3);
    let _ = define_host_data_property_with_env(ctx, env, obj, "name", name_val);
    let _ = define_host_data_property_with_env(ctx, env, obj, "message", message_val);
    // C2: 隐藏品牌
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


fn mark_promise_handle(
    caller: &mut Caller<'_, RuntimeState>,
    promise: i64,
    obj_table_ptr: usize,
    obj_table_count: usize,
) {
    if !value::is_object(promise) {
        return;
    }
    let handle_idx = raw_promise_handle(promise);
    if handle_idx >= obj_table_count {
        return;
    }
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return;
    };
    let data = memory.data(&*caller);
    let slot_addr = obj_table_ptr + handle_idx * 4;
    if slot_addr + 4 > data.len() {
        return;
    }
    let obj_ptr = u32::from_le_bytes([
        data[slot_addr],
        data[slot_addr + 1],
        data[slot_addr + 2],
        data[slot_addr + 3],
    ]) as usize;
    if obj_ptr != 0 {
        mark_object_recursive(caller, handle_idx, obj_ptr, obj_table_ptr, obj_table_count);
    }
}

pub(crate) fn trace_native_callable_record(
    caller: &mut Caller<'_, RuntimeState>,
    record: &NativeCallable,
    obj_table_ptr: usize,
    obj_table_count: usize,
    num_ir_functions: usize,
) {
    match record {
        NativeCallable::PromiseResolvingFunction { promise, .. } => {
            mark_promise_handle(caller, *promise, obj_table_ptr, obj_table_count);
        }
        NativeCallable::PromiseCombinatorReaction { context, .. } => {
            let (rp, ra) = {
                let state = caller.data();
                let contexts = state.combinator_contexts.lock().expect("combinator context mutex");
                contexts
                    .get(*context)
                    .map(|e| (e.result_promise, e.result_array))
                    .unwrap_or((value::encode_undefined(), value::encode_undefined()))
            };
            mark_runtime_value_recursive(
                caller,
                rp,
                obj_table_ptr,
                obj_table_count,
                num_ir_functions,
            );
            mark_runtime_value_recursive(
                caller,
                ra,
                obj_table_ptr,
                obj_table_count,
                num_ir_functions,
            );
        }
        NativeCallable::AsyncGeneratorMethod { generator, .. }
        | NativeCallable::AsyncGeneratorIdentity { generator } => {
            mark_runtime_value_recursive(
                caller,
                *generator,
                obj_table_ptr,
                obj_table_count,
                num_ir_functions,
            );
        }
        NativeCallable::EvalFunction(function) => {
            if let Some(env) = function.scope_env {
                mark_runtime_value_recursive(
                    caller,
                    env,
                    obj_table_ptr,
                    obj_table_count,
                    num_ir_functions,
                );
            }
        }
        NativeCallable::ProxyRevoker { proxy_handle } => {
            let _ = proxy_handle;
        }
        NativeCallable::HeadersMethod { .. }
        | NativeCallable::ResponseMethod { .. }
        | NativeCallable::RequestMethod { .. }
        | NativeCallable::ReadableStreamMethod { .. }
        | NativeCallable::ReadableStreamDefaultReaderMethod { .. }
        | NativeCallable::ReadableStreamDefaultControllerMethod { .. }
        | NativeCallable::ReadableStreamAsyncIteratorNext { .. }
        | NativeCallable::ReadableStreamAsyncIteratorReturn { .. }
        | NativeCallable::WritableStreamMethod { .. }
        | NativeCallable::WritableStreamDefaultWriterMethod { .. }
        | NativeCallable::WritableStreamDefaultControllerMethod { .. }
        | NativeCallable::TransformStreamMethod { .. }
        | NativeCallable::AbortControllerAbort { .. }
        | NativeCallable::AsyncFromSyncNext { .. }
        | NativeCallable::AsyncFromSyncReturn { .. }
        | NativeCallable::AsyncFromSyncThrow { .. } => {}
        _ => {}
    }
}

pub(crate) fn mark_runtime_value_recursive(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
    obj_table_ptr: usize,
    obj_table_count: usize,
    num_ir_functions: usize,
) {
    if value::is_object(val) || value::is_array(val) {
        let handle_idx = value::decode_object_handle(val) as usize;
        if handle_idx < obj_table_count {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return;
            };
            let data = memory.data(&*caller);
            let slot_addr = obj_table_ptr + handle_idx * 4;
            if slot_addr + 4 <= data.len() {
                let obj_ptr = u32::from_le_bytes([
                    data[slot_addr],
                    data[slot_addr + 1],
                    data[slot_addr + 2],
                    data[slot_addr + 3],
                ]) as usize;
                if obj_ptr != 0 {
                    mark_object_recursive(
                        caller,
                        handle_idx,
                        obj_ptr,
                        obj_table_ptr,
                        obj_table_count,
                    );
                }
            }
        }
        return;
    }
    if value::is_function(val) {
        let func_idx = (val as u64 & 0xFFFF_FFFF) as usize;
        if func_idx < num_ir_functions {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return;
            };
            let data = memory.data(&*caller);
            let slot_addr = obj_table_ptr + func_idx * 4;
            if slot_addr + 4 <= data.len() {
                let obj_ptr = u32::from_le_bytes([
                    data[slot_addr],
                    data[slot_addr + 1],
                    data[slot_addr + 2],
                    data[slot_addr + 3],
                ]) as usize;
                if obj_ptr != 0 {
                    mark_object_recursive(
                        caller,
                        func_idx,
                        obj_ptr,
                        obj_table_ptr,
                        obj_table_count,
                    );
                }
            }
        }
        return;
    }
    if value::is_closure(val) {
        let closure_idx = value::decode_closure_idx(val) as usize;
        let env = caller
            .data()
            .closures
            .lock()
            .expect("closures")
            .get(closure_idx)
            .map(|e| e.env_obj);
        if let Some(env) = env {
            mark_runtime_value_recursive(
                caller,
                env,
                obj_table_ptr,
                obj_table_count,
                num_ir_functions,
            );
        }
        return;
    }
    if value::is_native_callable(val) {
        let idx = value::decode_native_callable_idx(val) as usize;
        let record = caller
            .data()
            .native_callables
            .lock()
            .expect("native callable table mutex")
            .get(idx)
            .cloned();
        if let Some(record) = record {
            trace_native_callable_record(
                caller,
                &record,
                obj_table_ptr,
                obj_table_count,
                num_ir_functions,
            );
        }
    }
}

fn collect_child_from_value(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
    obj_table_ptr: usize,
    obj_table_count: usize,
    num_ir_functions: usize,
    children: &mut Vec<(usize, usize)>,
) {
    if value::is_object(val) || value::is_array(val) {
        let child_handle_idx = value::decode_object_handle(val) as usize;
        if child_handle_idx < obj_table_count {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return;
            };
            let data = memory.data(&*caller);
            let child_slot_addr = obj_table_ptr + child_handle_idx * 4;
            if child_slot_addr + 4 <= data.len() {
                let child_ptr = u32::from_le_bytes([
                    data[child_slot_addr],
                    data[child_slot_addr + 1],
                    data[child_slot_addr + 2],
                    data[child_slot_addr + 3],
                ]) as usize;
                if child_ptr != 0 {
                    children.push((child_handle_idx, child_ptr));
                }
            }
        }
        return;
    }
    if value::is_function(val) {
        let func_idx = (val as u64 & 0xFFFF_FFFF) as usize;
        if func_idx < num_ir_functions {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return;
            };
            let data = memory.data(&*caller);
            let child_slot_addr = obj_table_ptr + func_idx * 4;
            if child_slot_addr + 4 <= data.len() {
                let child_ptr = u32::from_le_bytes([
                    data[child_slot_addr],
                    data[child_slot_addr + 1],
                    data[child_slot_addr + 2],
                    data[child_slot_addr + 3],
                ]) as usize;
                if child_ptr != 0 {
                    children.push((func_idx, child_ptr));
                }
            }
        }
        return;
    }
    if value::is_closure(val) || value::is_native_callable(val) {
        mark_runtime_value_recursive(
            caller,
            val,
            obj_table_ptr,
            obj_table_count,
            num_ir_functions,
        );
    }
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
    mark_object_recursive_with_funcs(caller, handle_idx, obj_ptr, obj_table_ptr, obj_table_count, usize::MAX);
}

fn mark_object_recursive_with_funcs(
    caller: &mut Caller<'_, RuntimeState>,
    handle_idx: usize,
    obj_ptr: usize,
    obj_table_ptr: usize,
    obj_table_count: usize,
    num_ir_functions: usize,
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
    let mut pending_vals: Vec<i64> = Vec::new();

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
                pending_vals.push(elem);
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

                pending_vals.extend([value, getter, setter]);
            }
        }
    } // data 借用在这里结束

    for val in pending_vals {
        collect_child_from_value(
            caller,
            val,
            obj_table_ptr,
            obj_table_count,
            num_ir_functions,
            &mut children_to_mark,
        );
    }

    // 递归标记收集到的对象
    for (child_handle_idx, child_ptr) in children_to_mark {
        mark_object_recursive_with_funcs(
            caller,
            child_handle_idx,
            child_ptr,
            obj_table_ptr,
            obj_table_count,
            num_ir_functions,
        );
    }
}
