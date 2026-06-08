use super::*;
use crate::wasm_env::WasmEnv;
use std::sync::atomic::Ordering;

pub(crate) fn read_shadow_arg_with_env<C: AsContext>(
    ctx: &C,
    env: &WasmEnv,
    args_base: i32,
    index: u32,
) -> i64 {
    let data = env.memory.data(ctx);
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
        if value::is_closure(resolved)
            || value::is_function(resolved)
            || value::is_native_callable(resolved)
        {
            break;
        }
        if value::is_bound(resolved) {
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
    if value::is_native_callable(resolved) {
        // 先恢复 shadow_sp，因为 native_callable 不走 WASM 调用路径
        let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
        return call_native_callable_with_args_from_caller(
            &mut *caller,
            resolved,
            this_val,
            args.to_vec(),
        )
        .ok_or_else(|| anyhow::anyhow!("native callable returned None"));
    }
    if value::is_bound(resolved) {
        let bound_idx = value::decode_bound_idx(resolved) as usize;
        let (bound_func, bound_this, bound_args) = {
            let bound = caller.data().bound_objects.lock().unwrap();
            let record = &bound[bound_idx];
            (
                record.target_func,
                record.bound_this,
                record.bound_args.clone(),
            )
        };
        // 先恢复 shadow_sp
        let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
        // 合并 bound_args 和 args
        let mut combined_args = bound_args;
        combined_args.extend_from_slice(args);
        return call_wasm_callback(&mut *caller, bound_func, bound_this, &combined_args);
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
    let previous_new_target = caller
        .data()
        .new_target
        .swap(value::encode_undefined(), Ordering::Relaxed);
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
    caller
        .data()
        .new_target
        .store(previous_new_target, Ordering::Relaxed);
    let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
    call_result?;
    Ok(results[0].unwrap_i64())
}

/// Phase 3 must-convert 之 host reentrant 路径（按 2026-05-31-async-scheduler-implementation-plan.md 审计条目 + 26-async-audit-refactor-design.md）：
/// 为 `call_wasm_callback`（中央 host reentrant 调用点，proxy/define/array 等 13+ callers）添加 async 版本，与现有 sync `call_wasm_callback` 并存。
///
/// 规则：
/// - 严格与 sync 版本并存，供保留的 sync execute 路径继续使用
/// - 所有 bound/closure/proxy 解析逻辑、shadow stack 更新、native callable 短路、结果处理必须 100% 相同
/// - 仅 Wasm invocation（func table dispatch） + 返回值处理完全等价；唯一差异是将 `func.call(...)` 替换为 `func.call_async(...).await`
/// - 本阶段保持调用点不变（runtime_host_helpers 内部递归及所有 host_imports 调用仍使用 sync 版本；未来当 async host fn 路径激活时同步转换调用点）
/// - 精确保留原有行为，无任何语义或顺序差异
///
/// 特别提醒（plan Correction 3 + lib.rs 已有注释 + 审计计划）：
///   在 Store::epoch_deadline_async_yield_and_update 之后，
///   *所有* 经由该 Store 的 Wasm 调用（主 + 回调，包括此处 host reentrant 中的 func table 调用）都必须走 async API（call_async 等）。
///   本文件中的 async 版本即为此准备；sync 版本仅留给未切换的 sync execute 路径。
pub(crate) async fn call_wasm_callback_async(
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
        if value::is_closure(resolved)
            || value::is_function(resolved)
            || value::is_native_callable(resolved)
        {
            break;
        }
        if value::is_bound(resolved) {
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
    if value::is_native_callable(resolved) {
        // 先恢复 shadow_sp，因为 native_callable 不走 WASM 调用路径
        let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
        return Box::pin(call_native_callable_with_args_from_caller_async(
            &mut *caller,
            resolved,
            this_val,
            args.to_vec(),
        ))
        .await
        .ok_or_else(|| anyhow::anyhow!("native callable returned None"));
    }
    if value::is_bound(resolved) {
        let bound_idx = value::decode_bound_idx(resolved) as usize;
        let (bound_func, bound_this, bound_args) = {
            let bound = caller.data().bound_objects.lock().unwrap();
            let record = &bound[bound_idx];
            (
                record.target_func,
                record.bound_this,
                record.bound_args.clone(),
            )
        };
        // 先恢复 shadow_sp
        let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
        // 合并 bound_args 和 args
        let mut combined_args = bound_args;
        combined_args.extend_from_slice(args);
        return Box::pin(call_wasm_callback_async(
            &mut *caller,
            bound_func,
            bound_this,
            &combined_args,
        ))
        .await;
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
    let previous_new_target = caller
        .data()
        .new_target
        .swap(value::encode_undefined(), Ordering::Relaxed);
    let mut results = [Val::I64(0)];
    let call_result = func
        .call_async(
            &mut *caller,
            &[
                Val::I64(env_obj),
                Val::I64(this_val),
                Val::I32(shadow_sp),
                Val::I32(args.len() as i32),
            ],
            &mut results,
        )
        .await;
    // 恢复调用上下文（无论 call 成功与否）
    caller
        .data()
        .new_target
        .store(previous_new_target, Ordering::Relaxed);
    let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
    call_result?;
    Ok(results[0].unwrap_i64())
}

// ── 辅助函数：分配新数组 ────────────────────────────────────────
pub(crate) fn alloc_array_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let size = 16u32.saturating_add(capacity.saturating_mul(8));
    let new_heap_ptr = heap_ptr.saturating_add(size);
    let proto = env.array_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);
    let d = env.memory.data_mut(&mut *ctx);
    let ptr = heap_ptr as usize;
    if (new_heap_ptr as usize) > d.len() {
        return value::encode_undefined();
    }
    let slot_addr = obj_table_ptr as usize + obj_table_count as usize * 4;
    // obj_table 槽位耗尽时必须直接返回 undefined，不递增 obj_table_count、
    // 不前进 heap_ptr，保持 handle->slot 映射与 obj_table_count 一致。
    if slot_addr + 4 > d.len() {
        return value::encode_undefined();
    }
    d[ptr..ptr + 4].copy_from_slice(&proto.to_le_bytes());
    d[ptr + 4] = 1u8;
    d[ptr + 5..ptr + 8].fill(0);
    d[ptr + 8..ptr + 12].copy_from_slice(&0u32.to_le_bytes());
    d[ptr + 12..ptr + 16].copy_from_slice(&capacity.to_le_bytes());
    d[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
    let _ = d;
    let _ = env.heap_ptr.set(&mut *ctx, Val::I32(new_heap_ptr as i32));
    let _ = env
        .obj_table_count
        .set(&mut *ctx, Val::I32((obj_table_count + 1) as i32));
    value::encode_handle(value::TAG_ARRAY, obj_table_count)
}
// ── arr_proto_push (#49) ──────────────────────────────────────────
/// 从 host 元素设置数组元素（直接写入堆内存）
pub(crate) fn set_array_elem_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    arr_val: i64,
    index: i32,
    val: i64,
) {
    if !value::is_array(arr_val) {
        return;
    }
    let handle = value::decode_handle(arr_val) as usize;
    let Some(ptr) = resolve_handle_idx_with_env(ctx, env, handle) else {
        return;
    };
    let data = env.memory.data_mut(&mut *ctx);
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
pub(crate) fn alloc_object_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let size = 16u32.saturating_add(capacity.saturating_mul(32));
    let new_heap_ptr = heap_ptr.saturating_add(size);
    let d = env.memory.data_mut(&mut *ctx);
    let ptr = heap_ptr as usize;
    if (new_heap_ptr as usize) > d.len() {
        return value::encode_undefined();
    }
    let slot_addr = obj_table_ptr as usize + obj_table_count as usize * 4;
    // obj_table 槽位耗尽时必须直接返回 undefined，不递增 obj_table_count、
    // 不前进 heap_ptr，保持 handle->slot 映射与 obj_table_count 一致。
    if slot_addr + 4 > d.len() {
        return value::encode_undefined();
    }
    d[ptr..ptr + 4].copy_from_slice(&0u32.to_le_bytes()); // proto = 0 (null)
    d[ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT;
    d[ptr + 5..ptr + 8].fill(0);
    d[ptr + 8..ptr + 12].copy_from_slice(&capacity.to_le_bytes()); // capacity
    d[ptr + 12..ptr + 16].copy_from_slice(&0u32.to_le_bytes()); // num_props = 0
    d[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
    let _ = d;
    let _ = env.heap_ptr.set(&mut *ctx, Val::I32(new_heap_ptr as i32));
    let _ = env
        .obj_table_count
        .set(&mut *ctx, Val::I32((obj_table_count + 1) as i32));
    value::encode_object_handle(obj_table_count)
}

pub(crate) fn find_memory_c_string_with_env<C: AsContext>(
    ctx: &C,
    env: &WasmEnv,
    name: &str,
) -> Option<u32> {
    let mut needle = Vec::with_capacity(name.len() + 1);
    needle.extend_from_slice(name.as_bytes());
    needle.push(0);
    env.memory
        .data(ctx)
        .windows(needle.len())
        .position(|window| window == needle.as_slice())
        .map(|offset| offset as u32)
}

pub(crate) fn alloc_heap_c_string_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    name: &str,
) -> Option<u32> {
    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as usize;
    let bytes = name.as_bytes();
    let end = heap_ptr.checked_add(bytes.len() + 1)?;
    let aligned_end = (end + 7) & !7;
    {
        let data = env.memory.data_mut(&mut *ctx);
        if aligned_end > data.len() {
            return None;
        }
        data[heap_ptr..heap_ptr + bytes.len()].copy_from_slice(bytes);
        data[heap_ptr + bytes.len()] = 0;
        data[end..aligned_end].fill(0);
    }
    let _ = env.heap_ptr.set(&mut *ctx, Val::I32(aligned_end as i32));
    Some(heap_ptr as u32)
}

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
    if value::is_function(val)
        || value::is_closure(val)
        || value::is_bound(val)
        || value::is_native_callable(val)
    {
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

pub(crate) fn is_constructor_in_runtime(caller: &mut Caller<'_, RuntimeState>, val: i64) -> bool {
    if value::is_function(val)
        || value::is_closure(val)
        || value::is_bound(val)
        || value::is_native_callable(val)
    {
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
                return is_constructor_in_runtime(caller, entry.target);
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
    let set = caller
        .data()
        .non_extensible_handles
        .lock()
        .expect("non_extensible_handles mutex");
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
    let mut set = caller
        .data()
        .non_extensible_handles
        .lock()
        .expect("non_extensible_handles mutex");
    set.insert(target as u64);
    true
}
pub(crate) fn prototype_handle_to_value(
    caller: &mut Caller<'_, RuntimeState>,
    proto_handle: u32,
) -> i64 {
    if proto_handle == 0xFFFF_FFFF {
        return value::encode_null();
    }
    let num_ir_functions = caller
        .get_export("__num_ir_functions")
        .and_then(Extern::into_global)
        .and_then(|global| global.get(&mut *caller).i32())
        .unwrap_or(0) as u32;
    if proto_handle < num_ir_functions {
        value::encode_function_idx(proto_handle)
    } else {
        value::encode_object_handle(proto_handle)
    }
}

pub(crate) fn proxy_or_target_get_prototype_of_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
) -> i64 {
    if value::is_proxy(target) {
        let handle = value::decode_proxy_handle(target) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Cannot perform 'getPrototypeOf' on a proxy that has been revoked"
                        .to_string(),
                );
                return value::encode_undefined();
            }
            if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                let trap = read_object_property_by_name(caller, handler_ptr, "getPrototypeOf")
                    .unwrap_or_else(value::encode_undefined);
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    let result =
                        match call_wasm_callback(caller, trap, entry.handler, &[entry.target]) {
                            Ok(result) => result,
                            Err(error) => {
                                set_runtime_error(
                                    caller.data(),
                                    format!("TypeError: getPrototypeOf trap failed: {error}"),
                                );
                                return value::encode_null();
                            }
                        };
                    if !value::is_null(result) && !value::is_js_object(result) {
                        set_runtime_error(
                            caller.data(),
                            "TypeError: Proxy getPrototypeOf must return an object or null"
                                .to_string(),
                        );
                        return value::encode_null();
                    }
                    if !is_extensible_impl(caller, entry.target) {
                        let target_proto =
                            proxy_or_target_get_prototype_of_impl(caller, entry.target);
                        if result != target_proto {
                            set_runtime_error(
                                caller.data(),
                                "TypeError: Proxy getPrototypeOf invariant violated: target is not extensible and trap returned different prototype".to_string(),
                            );
                            return value::encode_null();
                        }
                    }
                    return result;
                }
            }
            return proxy_or_target_get_prototype_of_impl(caller, entry.target);
        }
    }

    let Some(ptr) = resolve_handle(caller, target) else {
        return value::encode_null();
    };
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return value::encode_null();
    };
    let data = memory.data(&*caller);
    if ptr + 4 > data.len() {
        return value::encode_null();
    }
    let proto_handle = u32::from_le_bytes([data[ptr], data[ptr + 1], data[ptr + 2], data[ptr + 3]]);
    if proto_handle == 0 && value::is_object(target) {
        return value::encode_null();
    }
    prototype_handle_to_value(caller, proto_handle)
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
        if !value::is_undefined(getter)
            && !value::is_null(getter)
            && !is_callable_in_runtime(caller, getter)
        {
            return Err("TypeError: property getter must be callable".to_string());
        }
    }
    if let Some(setter) = prop_set {
        if !value::is_undefined(setter)
            && !value::is_null(setter)
            && !is_callable_in_runtime(caller, setter)
        {
            return Err("TypeError: property setter must be callable".to_string());
        }
    }

    let has_accessor = prop_get.is_some() || prop_set.is_some();
    if has_accessor {
        if prop_value.is_some() {
            return Err(
                "TypeError: Invalid property descriptor: cannot specify both accessor and value"
                    .to_string(),
            );
        }
        if prop_writable.is_some() {
            return Err(
                "TypeError: Invalid property descriptor: cannot specify both accessor and writable"
                    .to_string(),
            );
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
        let getter =
            i64::from_le_bytes(data[slot_offset + 16..slot_offset + 24].try_into().unwrap());
        let setter =
            i64::from_le_bytes(data[slot_offset + 24..slot_offset + 32].try_into().unwrap());
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

fn is_accessor_descriptor(desc: &PropertyDescriptor) -> bool {
    desc.get.is_some() || desc.set.is_some()
}

fn is_data_descriptor(desc: &PropertyDescriptor) -> bool {
    desc.value.is_some() || desc.writable.is_some()
}

fn complete_property_descriptor(mut desc: PropertyDescriptor) -> PropertyDescriptor {
    if is_accessor_descriptor(&desc) {
        desc.get.get_or_insert_with(value::encode_undefined);
        desc.set.get_or_insert_with(value::encode_undefined);
    } else {
        desc.value.get_or_insert_with(value::encode_undefined);
        desc.writable.get_or_insert(false);
    }
    desc.enumerable.get_or_insert(false);
    desc.configurable.get_or_insert(false);
    desc
}

fn descriptor_value_same(caller: &mut Caller<'_, RuntimeState>, left: i64, right: i64) -> bool {
    !value::is_falsy(strict_eq(caller, left, right))
}

fn is_compatible_property_descriptor(
    caller: &mut Caller<'_, RuntimeState>,
    extensible: bool,
    desc: &PropertyDescriptor,
    current: Option<&PropertyDescriptor>,
) -> bool {
    let Some(current) = current else {
        return extensible;
    };

    let current_configurable = current.configurable.unwrap_or(false);
    if !current_configurable {
        if desc.configurable == Some(true) {
            return false;
        }
        if desc.enumerable != current.enumerable {
            return false;
        }
    }

    let current_is_data = is_data_descriptor(current);
    let desc_is_data = is_data_descriptor(desc);
    if current_is_data != desc_is_data {
        return current_configurable;
    }

    if current_is_data {
        if !current_configurable && current.writable == Some(false) {
            if desc.writable == Some(true) {
                return false;
            }
            let current_value = current.value.unwrap_or_else(value::encode_undefined);
            let desc_value = desc.value.unwrap_or_else(value::encode_undefined);
            if !descriptor_value_same(caller, current_value, desc_value) {
                return false;
            }
        }
        return true;
    }

    if !current_configurable {
        if desc.get != current.get {
            return false;
        }
        if desc.set != current.set {
            return false;
        }
    }
    true
}

pub(crate) fn validate_proxy_get_own_property_descriptor_result(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: Option<u32>,
    trap_result: i64,
) -> Result<(), String> {
    let target_desc = name_id.and_then(|id| get_target_descriptor(caller, target, id));
    let extensible = is_extensible_impl(caller, target);

    if value::is_undefined(trap_result) {
        let Some(target_desc) = target_desc else {
            return Ok(());
        };
        if target_desc.configurable == Some(false) {
            return Err("TypeError: Proxy getOwnPropertyDescriptor invariant violated: non-configurable property must not be reported as undefined".to_string());
        }
        if !extensible {
            return Err("TypeError: Proxy getOwnPropertyDescriptor invariant violated: target is non-extensible and property cannot be reported as missing".to_string());
        }
        return Ok(());
    }

    if !value::is_js_object(trap_result) {
        return Err(
            "TypeError: Proxy getOwnPropertyDescriptor trap must return an object or undefined"
                .to_string(),
        );
    }

    let result_desc = complete_property_descriptor(parse_descriptor(caller, trap_result)?);
    if !is_compatible_property_descriptor(caller, extensible, &result_desc, target_desc.as_ref()) {
        return Err("TypeError: Proxy getOwnPropertyDescriptor invariant violated: descriptor is incompatible with target".to_string());
    }

    if result_desc.configurable == Some(false) {
        let Some(target_desc) = target_desc.as_ref() else {
            return Err("TypeError: Proxy getOwnPropertyDescriptor invariant violated: non-configurable descriptor is incompatible with target".to_string());
        };
        if target_desc.configurable != Some(false) {
            return Err("TypeError: Proxy getOwnPropertyDescriptor invariant violated: non-configurable descriptor is incompatible with target".to_string());
        }
        if is_data_descriptor(target_desc)
            && target_desc.writable == Some(false)
            && result_desc.writable == Some(true)
        {
            return Err("TypeError: Proxy getOwnPropertyDescriptor invariant violated: non-configurable descriptor is incompatible with target".to_string());
        }
    }

    Ok(())
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
            let g =
                i64::from_le_bytes(data[slot_offset + 16..slot_offset + 24].try_into().unwrap());
            let s =
                i64::from_le_bytes(data[slot_offset + 24..slot_offset + 32].try_into().unwrap());
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
                        return Err(
                            "TypeError: Cannot make non-writable property writable".to_string()
                        );
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
                        return Err(
                            "TypeError: Cannot change getter of non-configurable property"
                                .to_string(),
                        );
                    }
                }
                if let Some(new_setter) = desc.set {
                    if new_setter != old_setter {
                        return Err(
                            "TypeError: Cannot change setter of non-configurable property"
                                .to_string(),
                        );
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

            data[heap_ptr + 8..heap_ptr + 12].copy_from_slice(&(new_capacity as u32).to_le_bytes());

            let slot_addr = obj_table_ptr + handle_idx as usize * 4;
            if slot_addr + 4 <= data.len() {
                data[slot_addr..slot_addr + 4].copy_from_slice(&(heap_ptr as u32).to_le_bytes());
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

pub(crate) async fn proxy_or_target_get_prototype_of_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
) -> i64 {
    if value::is_proxy(target) {
        let handle = value::decode_proxy_handle(target) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Cannot perform 'getPrototypeOf' on a proxy that has been revoked"
                        .to_string(),
                );
                return value::encode_undefined();
            }
            if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                let trap = read_object_property_by_name(caller, handler_ptr, "getPrototypeOf")
                    .unwrap_or_else(value::encode_undefined);
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    let result = match call_wasm_callback_async(
                        caller,
                        trap,
                        entry.handler,
                        &[entry.target],
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(error) => {
                            set_runtime_error(
                                caller.data(),
                                format!("TypeError: getPrototypeOf trap failed: {error}"),
                            );
                            return value::encode_null();
                        }
                    };
                    if !value::is_null(result) && !value::is_js_object(result) {
                        set_runtime_error(
                            caller.data(),
                            "TypeError: Proxy getPrototypeOf must return an object or null"
                                .to_string(),
                        );
                        return value::encode_null();
                    }
                    if !is_extensible_impl(caller, entry.target) {
                        let target_proto = Box::pin(proxy_or_target_get_prototype_of_impl_async(
                            caller,
                            entry.target,
                        ))
                        .await;
                        if result != target_proto {
                            set_runtime_error(
                                caller.data(),
                                "TypeError: Proxy getPrototypeOf invariant violated: target is not extensible and trap returned different prototype".to_string(),
                            );
                            return value::encode_null();
                        }
                    }
                    return result;
                }
            }
            return Box::pin(proxy_or_target_get_prototype_of_impl_async(
                caller,
                entry.target,
            ))
            .await;
        }
    }

    let Some(ptr) = resolve_handle(caller, target) else {
        return value::encode_null();
    };
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return value::encode_null();
    };
    let data = memory.data(&*caller);
    if ptr + 4 > data.len() {
        return value::encode_null();
    }
    let proto_handle = u32::from_le_bytes([data[ptr], data[ptr + 1], data[ptr + 2], data[ptr + 3]]);
    if proto_handle == 0 && value::is_object(target) {
        return value::encode_null();
    }
    prototype_handle_to_value(caller, proto_handle)
}

pub(crate) async fn proxy_or_target_is_extensible_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
) -> bool {
    if value::is_proxy(target) {
        let handle = value::decode_proxy_handle(target) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Cannot perform 'isExtensible' on a proxy that has been revoked"
                        .to_string(),
                );
                return false;
            }
            if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                let trap = read_object_property_by_name(caller, handler_ptr, "isExtensible")
                    .unwrap_or_else(value::encode_undefined);
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    let trap_res = match call_wasm_callback_async(
                        caller,
                        trap,
                        entry.handler,
                        &[entry.target],
                    )
                    .await
                    {
                        Ok(result) => !value::is_falsy(result),
                        Err(error) => {
                            set_runtime_error(
                                caller.data(),
                                format!("TypeError: isExtensible trap failed: {error}"),
                            );
                            return false;
                        }
                    };
                    let real_res = is_extensible_impl(caller, entry.target);
                    if trap_res != real_res {
                        set_runtime_error(
                            caller.data(),
                            "TypeError: Proxy isExtensible trap returned result that does not match target's extensibility".to_string(),
                        );
                        return false;
                    }
                    return trap_res;
                }
            }
            return is_extensible_impl(caller, entry.target);
        }
    }
    is_extensible_impl(caller, target)
}

pub(crate) async fn proxy_or_target_prevent_extensions_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
) -> bool {
    if value::is_proxy(target) {
        let handle = value::decode_proxy_handle(target) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Cannot perform 'preventExtensions' on a proxy that has been revoked".to_string(),
                );
                return false;
            }
            if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                let trap = read_object_property_by_name(caller, handler_ptr, "preventExtensions")
                    .unwrap_or_else(value::encode_undefined);
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    let trap_res = match call_wasm_callback_async(
                        caller,
                        trap,
                        entry.handler,
                        &[entry.target],
                    )
                    .await
                    {
                        Ok(result) => !value::is_falsy(result),
                        Err(error) => {
                            set_runtime_error(
                                caller.data(),
                                format!("TypeError: preventExtensions trap failed: {error}"),
                            );
                            return false;
                        }
                    };
                    if trap_res && is_extensible_impl(caller, entry.target) {
                        set_runtime_error(
                            caller.data(),
                            "TypeError: Proxy preventExtensions trap returned true, but target remains extensible".to_string(),
                        );
                        return false;
                    }
                    return trap_res;
                }
            }
            return prevent_extensions_impl(caller, entry.target);
        }
    }
    prevent_extensions_impl(caller, target)
}

pub(crate) async fn define_property_internal_async(
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

    define_property_on_target_async(caller, target, name_id, prop, descriptor, &desc).await
}

async fn define_property_on_target_async(
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
            return Err(
                "TypeError: Cannot perform 'defineProperty' on a proxy that has been revoked"
                    .to_string(),
            );
        }

        if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
            let trap = read_object_property_by_name(caller, handler_ptr, "defineProperty")
                .unwrap_or_else(value::encode_undefined);
            if !value::is_undefined(trap) && !value::is_null(trap) {
                let result = match call_wasm_callback_async(
                    caller,
                    trap,
                    entry.handler,
                    &[entry.target, prop_val, descriptor],
                )
                .await
                {
                    Ok(res) => res,
                    Err(e) => return Err(format!("TypeError: defineProperty trap failed: {}", e)),
                };
                let trap_result = !value::is_falsy(result);
                if !trap_result {
                    return Ok(false);
                }

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

                return Box::pin(define_property_on_target_async(
                    caller,
                    entry.target,
                    name_id,
                    prop_val,
                    descriptor,
                    desc,
                ))
                .await;
            }
        }

        return Box::pin(define_property_on_target_async(
            caller,
            entry.target,
            name_id,
            prop_val,
            descriptor,
            desc,
        ))
        .await;
    }

    define_property_on_normal_object(caller, target, name_id, desc)
}

pub(crate) fn reflect_get_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
) -> i64 {
    reflect_get_impl_with_receiver(caller, target, prop, target)
}

pub(crate) fn reflect_get_impl_with_receiver(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
    receiver: i64,
) -> i64 {
    if value::is_proxy(target) {
        let handle = value::decode_proxy_handle(target) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Cannot perform 'get' on a proxy that has been revoked".to_string(),
                );
                return value::encode_undefined();
            }
            if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                let trap = read_object_property_by_name(caller, handler_ptr, "get")
                    .unwrap_or_else(value::encode_undefined);
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    return call_wasm_callback(
                        caller,
                        trap,
                        entry.handler,
                        &[entry.target, prop, receiver],
                    )
                    .unwrap_or_else(|_| value::encode_undefined());
                }
            }
            return reflect_get_impl_with_receiver(caller, entry.target, prop, receiver);
        }
        return value::encode_undefined();
    }

    let prop_name = match render_value(caller, prop) {
        Ok(name) => name,
        Err(_) => return value::encode_undefined(),
    };
    let obj_ptr = match resolve_handle(caller, target) {
        Some(ptr) => ptr,
        None => return value::encode_undefined(),
    };
    let name_id = find_memory_c_string(caller, &prop_name);

    if prop_name == "prototype"
        && (value::is_function(target) || value::is_closure(target) || value::is_bound(target))
    {
        if let Some(id) = name_id
            && let Some((_, _, value)) = find_property_slot_by_name_id(caller, obj_ptr, id)
            && !value::is_undefined(value)
        {
            return value;
        }

        // 函数首次读取 prototype 时按需创建默认 prototype，并写回函数对象。
        let default_proto = {
            let wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &wjsm_env, 4)
        };
        let _ = define_host_data_property_from_caller(caller, target, "prototype", default_proto);
        if let Some(name_c) = find_memory_c_string(caller, "constructor")
            && let Some(dp_ptr) = resolve_handle(caller, default_proto)
        {
            write_object_property_by_name_id(
                caller,
                dp_ptr,
                default_proto,
                name_c as u32,
                target,
                constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
            );
        }
        return default_proto;
    }

    if let Some(id) = name_id
        && let Some((slot_offset, flags, value)) =
            find_property_slot_by_name_id(caller, obj_ptr, id)
    {
        if (flags & constants::FLAG_IS_ACCESSOR) == 0 {
            return value;
        }
        let getter = {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return value::encode_undefined();
            };
            let data = memory.data(&*caller);
            if slot_offset + 24 > data.len() {
                return value::encode_undefined();
            }
            i64::from_le_bytes(data[slot_offset + 16..slot_offset + 24].try_into().unwrap())
        };
        if value::is_undefined(getter) || value::is_null(getter) {
            return value::encode_undefined();
        }
        return call_wasm_callback(caller, getter, receiver, &[])
            .unwrap_or_else(|_| value::encode_undefined());
    }

    let proto = proxy_or_target_get_prototype_of_impl(caller, target);
    if value::is_null(proto) {
        value::encode_undefined()
    } else {
        reflect_get_impl_with_receiver(caller, proto, prop, receiver)
    }
}

pub(crate) async fn reflect_get_impl_with_receiver_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
    receiver: i64,
) -> i64 {
    if value::is_proxy(target) {
        let handle = value::decode_proxy_handle(target) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Cannot perform 'get' on a proxy that has been revoked".to_string(),
                );
                return value::encode_undefined();
            }
            if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                let trap = read_object_property_by_name(caller, handler_ptr, "get")
                    .unwrap_or_else(value::encode_undefined);
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    return call_wasm_callback_async(
                        caller,
                        trap,
                        entry.handler,
                        &[entry.target, prop, receiver],
                    )
                    .await
                    .unwrap_or_else(|_| value::encode_undefined());
                }
            }
            return Box::pin(reflect_get_impl_with_receiver_async(
                caller,
                entry.target,
                prop,
                receiver,
            ))
            .await;
        }
        return value::encode_undefined();
    }

    let prop_name = match render_value(caller, prop) {
        Ok(name) => name,
        Err(_) => return value::encode_undefined(),
    };
    let obj_ptr = match resolve_handle(caller, target) {
        Some(ptr) => ptr,
        None => return value::encode_undefined(),
    };
    let name_id = find_memory_c_string(caller, &prop_name);

    if prop_name == "prototype"
        && (value::is_function(target) || value::is_closure(target) || value::is_bound(target))
    {
        if let Some(id) = name_id
            && let Some((_, _, value)) = find_property_slot_by_name_id(caller, obj_ptr, id)
            && !value::is_undefined(value)
        {
            return value;
        }

        let default_proto = {
            let wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &wjsm_env, 4)
        };
        let _ = define_host_data_property_from_caller(caller, target, "prototype", default_proto);
        if let Some(name_c) = find_memory_c_string(caller, "constructor")
            && let Some(dp_ptr) = resolve_handle(caller, default_proto)
        {
            write_object_property_by_name_id(
                caller,
                dp_ptr,
                default_proto,
                name_c as u32,
                target,
                constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
            );
        }
        return default_proto;
    }

    if let Some(id) = name_id
        && let Some((slot_offset, flags, value)) =
            find_property_slot_by_name_id(caller, obj_ptr, id)
    {
        if (flags & constants::FLAG_IS_ACCESSOR) == 0 {
            return value;
        }
        let getter = {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return value::encode_undefined();
            };
            let data = memory.data(&*caller);
            if slot_offset + 24 > data.len() {
                return value::encode_undefined();
            }
            i64::from_le_bytes(data[slot_offset + 16..slot_offset + 24].try_into().unwrap())
        };
        if value::is_undefined(getter) || value::is_null(getter) {
            return value::encode_undefined();
        }
        return call_wasm_callback_async(caller, getter, receiver, &[])
            .await
            .unwrap_or_else(|_| value::encode_undefined());
    }

    let proto = proxy_or_target_get_prototype_of_impl_async(caller, target).await;
    if value::is_null(proto) {
        value::encode_undefined()
    } else {
        Box::pin(reflect_get_impl_with_receiver_async(
            caller, proto, prop, receiver,
        ))
        .await
    }
}

// ── Caller 双参数便捷入口（委托 WasmEnv 泛型实现）────────────────────

#[inline]
pub(crate) fn read_shadow_arg(
    caller: &mut Caller<'_, RuntimeState>,
    args_base: i32,
    index: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    read_shadow_arg_with_env(caller, &env, args_base, index)
}

#[inline]
pub(crate) fn alloc_array(caller: &mut Caller<'_, RuntimeState>, capacity: u32) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    alloc_array_with_env(caller, &env, capacity)
}

#[inline]
pub(crate) fn set_array_elem(
    caller: &mut Caller<'_, RuntimeState>,
    arr_val: i64,
    index: i32,
    val: i64,
) {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    set_array_elem_with_env(caller, &env, arr_val, index, val);
}

#[inline]
pub(crate) fn alloc_object(caller: &mut Caller<'_, RuntimeState>, capacity: u32) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    alloc_object_with_env(caller, &env, capacity)
}

#[inline]
pub(crate) fn alloc_promise(caller: &mut Caller<'_, RuntimeState>, entry: PromiseEntry) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let promise = alloc_object_with_env(caller, &env, 0);
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

#[inline]
pub(crate) fn find_memory_c_string(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
) -> Option<u32> {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    find_memory_c_string_with_env(caller, &env, name)
}

#[inline]
pub(crate) fn alloc_heap_c_string(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
) -> Option<u32> {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    alloc_heap_c_string_with_env(caller, &env, name)
}

#[inline]
pub(crate) fn define_host_data_property(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    val: i64,
) -> Option<()> {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let name_id = find_memory_c_string_with_env(caller, &env, name)
        .or_else(|| alloc_heap_c_string_with_env(caller, &env, name))?;
    define_host_data_property_by_name_id(caller, obj, encode_string_name_id(name_id), val)
}

pub(crate) fn define_host_data_property_symbol(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    symbol_val: i64,
    val: i64,
) -> Option<()> {
    let name_id = symbol_value_to_name_id(symbol_val)?;
    define_host_data_property_by_name_id(caller, obj, name_id, val)
}

pub(crate) fn define_host_data_property_by_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name_id: u32,
    val: i64,
) -> Option<()> {
    define_host_data_property_by_name_id_with_flags(
        caller,
        obj,
        name_id,
        val,
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE,
    )
}

pub(crate) fn define_host_data_property_by_name_id_with_flags(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name_id: u32,
    val: i64,
    flags: i32,
) -> Option<()> {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj_ptr =
        resolve_handle_idx_with_env(caller, &env, value::decode_object_handle(obj) as usize)?;
    let (capacity, num_props) = {
        let data = env.memory.data(&*caller);
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
    let data = env.memory.data_mut(&mut *caller);
    let slot_offset = actual_ptr + 16 + num_props as usize * 32;
    if slot_offset + 32 > data.len() {
        return None;
    }
    let undef = value::encode_undefined();
    data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
    data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
    data[slot_offset + 16..slot_offset + 24].copy_from_slice(&undef.to_le_bytes());
    data[slot_offset + 24..slot_offset + 32].copy_from_slice(&undef.to_le_bytes());
    data[actual_ptr + 12..actual_ptr + 16].copy_from_slice(&(num_props + 1).to_le_bytes());
    Some(())
}

/// 定义一个访问器（getter/setter）属性到宿主创建的对象上（Caller 版本，支持 grow_object）。
/// slot 布局与数据属性相同（32字节），但 flags 标记为 IS_ACCESSOR，
/// offset 8 = undefined（保留），offset 16 = getter，offset 24 = setter。
#[inline]
pub(crate) fn define_host_accessor_property(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    getter: i64,
    setter: i64,
) -> Option<()> {
    define_host_accessor_property_with_flags(
        caller,
        obj,
        name,
        getter,
        setter,
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE,
    )
}

pub(crate) fn define_host_accessor_property_symbol(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    symbol_val: i64,
    getter: i64,
    setter: i64,
) -> Option<()> {
    let name_id = symbol_value_to_name_id(symbol_val)?;
    define_host_accessor_property_by_name_id_with_flags(
        caller,
        obj,
        name_id,
        getter,
        setter,
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE,
    )
}

/// 定义可配置属性位的访问器属性。
pub(crate) fn define_host_accessor_property_with_flags(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    getter: i64,
    setter: i64,
    attribute_flags: i32,
) -> Option<()> {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let name_id = find_memory_c_string_with_env(caller, &env, name)
        .or_else(|| alloc_heap_c_string_with_env(caller, &env, name))?;
    define_host_accessor_property_by_name_id_with_flags(
        caller,
        obj,
        encode_string_name_id(name_id),
        getter,
        setter,
        attribute_flags,
    )
}

pub(crate) fn define_host_accessor_property_by_name_id_with_flags(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name_id: u32,
    getter: i64,
    setter: i64,
    attribute_flags: i32,
) -> Option<()> {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj_ptr =
        resolve_handle_idx_with_env(caller, &env, value::decode_object_handle(obj) as usize)?;
    let (capacity, num_props) = {
        let data = env.memory.data(&*caller);
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
    let data = env.memory.data_mut(&mut *caller);
    let slot_offset = actual_ptr + 16 + num_props as usize * 32;
    if slot_offset + 32 > data.len() {
        return None;
    }
    let flags = (attribute_flags & !constants::FLAG_WRITABLE) | constants::FLAG_IS_ACCESSOR;
    let undef = value::encode_undefined();
    data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
    data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    data[slot_offset + 8..slot_offset + 16].copy_from_slice(&undef.to_le_bytes());
    data[slot_offset + 16..slot_offset + 24].copy_from_slice(&getter.to_le_bytes());
    data[slot_offset + 24..slot_offset + 32].copy_from_slice(&setter.to_le_bytes());
    data[actual_ptr + 12..actual_ptr + 16].copy_from_slice(&(num_props + 1).to_le_bytes());
    Some(())
}
