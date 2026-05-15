use wasmtime::{Caller, Extern, Table, Val};

use crate::types::{RuntimeState, BoundRecord, NativeCallable, PromiseResolvingKind, PromiseEntry};
use crate::runtime::string_utils::store_runtime_string;
use crate::runtime::memory::{resolve_handle_idx, alloc_object, define_host_data_property, alloc_array, alloc_promise_all_settled_result, alloc_aggregate_error};
use crate::runtime::object_ops::{read_object_property_by_name, grow_object, collect_own_property_names, collect_own_property_values, allocate_descriptor_object};
use crate::runtime::conversions::{to_number, type_tag, get_string_value};
use crate::runtime::eval::{is_promise_value, promise_entry_mut, promise_entry, settle_promise, alloc_promise_from_caller, new_promise_capability_from_caller, create_promise_resolving_functions, runtime_error_value, alloc_iterator_result_from_caller};
use crate::runtime::microtask::{set_runtime_error, call_host_function_from_caller};
use wjsm_ir::{constants, value};

pub(crate) fn read_shadow_arg(caller: &mut Caller<'_, RuntimeState>, args_base: i32, index: u32) -> i64 {
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
    shadow_sp_global.set(&mut *caller, Val::I32(new_shadow_sp))?;
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
    let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
    call_result?;
    Ok(results[0].unwrap_i64())
}
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

pub(crate) fn object_rest_impl(_caller: &mut Caller<'_, RuntimeState>, _obj: i64, _excluded_keys: i64) -> i64 {
    // 简化实现：返回一个新的空对象
    // 完整实现需要遍历 obj 的属性并排除指定键
    value::encode_undefined()
}

pub(crate) fn obj_spread_impl(_caller: &mut Caller<'_, RuntimeState>, _dest: i64, _source: i64) {
    // 简化实现：不做任何复制
    // 完整实现需要遍历 source 的 own properties 并复制到 dest
}

#[derive(Clone, Copy)]
enum PromiseSettlement {
    Fulfill(i64),
    Reject(i64),
}

pub(crate) fn raw_promise_handle(promise: i64) -> usize {
    if value::is_object(promise) {
        value::decode_object_handle(promise) as usize
    } else {
        promise as usize
    }
}

pub(crate) fn insert_promise_entry(table: &mut Vec<PromiseEntry>, handle: usize, entry: PromiseEntry) {
    if table.len() <= handle {
        table.resize_with(handle + 1, PromiseEntry::empty);
    }
    table[handle] = entry;
}

