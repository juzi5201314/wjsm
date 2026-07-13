//! 目标 agent 上从 `SerializedValue` 重建 JS 值。

use std::collections::HashMap;
use std::str::FromStr;

use num_bigint::BigInt;
use wasmtime::{AsContext, AsContextMut};

use crate::runtime_buffer::create_buffer_from_bytes_with_env;
use crate::runtime_render::store_runtime_string_in_state;
use crate::runtime_worker_message::{MESSAGE_PORT_ID_PROP, SAB_HANDLE_PROP, SerializedValue};
use crate::*;

struct DeCtx {
    memo: HashMap<usize, i64>,
}

/// 在目标 agent 的 Store/Caller 上重建 JS 值。
pub(crate) fn deserialize_value<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    value: &SerializedValue,
) -> i64 {
    let mut cx = DeCtx {
        memo: HashMap::new(),
    };
    deserialize_one(ctx, env, value, &mut cx)
}

/// Caller 路径便捷封装：从当前 host 上下文取 `WasmEnv` 后反序列化。
#[allow(dead_code)]
pub(crate) fn deserialize_value_from_caller(
    caller: &mut wasmtime::Caller<'_, RuntimeState>,
    value: SerializedValue,
) -> i64 {
    let env = match WasmEnv::from_caller(caller) {
        Some(env) => env,
        None => return value::encode_undefined(),
    };
    deserialize_value(caller, &env, &value)
}

fn deserialize_one<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    value: &SerializedValue,
    cx: &mut DeCtx,
) -> i64 {
    match value {
        SerializedValue::Undefined => value::encode_undefined(),
        SerializedValue::Null => value::encode_null(),
        SerializedValue::Bool(b) => value::encode_bool(*b),
        SerializedValue::Number(n) => value::encode_f64(*n),
        SerializedValue::String(s) => {
            store_runtime_string_in_state(ctx.as_context().data(), s.clone())
        }
        SerializedValue::BigInt(s) => deserialize_bigint(ctx, s),
        SerializedValue::Ref(id) => cx
            .memo
            .get(id)
            .copied()
            .unwrap_or_else(value::encode_undefined),
        SerializedValue::Array { id, items } => {
            let arr = alloc_array_with_env(ctx, env, items.len() as u32);
            cx.memo.insert(*id, arr);
            let Some(ptr) = resolve_array_ptr_with_env(ctx, env, arr) else {
                return arr;
            };
            for (index, item) in items.iter().enumerate() {
                let v = deserialize_one(ctx, env, item, cx);
                write_array_elem_with_env(ctx, env, ptr, index as u32, v);
            }
            write_array_length_with_env(ctx, env, ptr, items.len() as u32);
            arr
        }
        SerializedValue::Object { id, entries } => {
            let obj = alloc_host_object(ctx, env, entries.len() as u32);
            cx.memo.insert(*id, obj);
            for (name, val) in entries {
                let v = deserialize_one(ctx, env, val, cx);
                let _ = define_host_data_property_with_env(ctx, env, obj, name, v);
            }
            obj
        }
        SerializedValue::Map { id, entries } => deserialize_map(ctx, env, *id, entries, cx),
        SerializedValue::Set { id, values } => deserialize_set(ctx, env, *id, values, cx),
        SerializedValue::Date { id, ms } => {
            let obj = create_date_with_env(ctx, env, *ms);
            cx.memo.insert(*id, obj);
            obj
        }
        SerializedValue::RegExp { id, source, flags } => {
            let obj = create_regexp_plain_with_env(ctx, env, source, flags);
            cx.memo.insert(*id, obj);
            obj
        }
        SerializedValue::ArrayBuffer { id, bytes } => {
            let obj = create_arraybuffer_from_bytes_with_env(ctx, env, bytes.clone());
            cx.memo.insert(*id, obj);
            obj
        }
        SerializedValue::Buffer { id, bytes } | SerializedValue::TypedArray { id, bytes, .. } => {
            let obj = create_buffer_from_bytes_with_env(ctx, env, bytes.clone());
            cx.memo.insert(*id, obj);
            obj
        }
        SerializedValue::SharedArrayBuffer { id, handle } => {
            let obj = materialize_sab_with_env(ctx, env, *handle);
            cx.memo.insert(*id, obj);
            obj
        }
        SerializedValue::MessagePort { id, global_id } => {
            let obj = create_message_port_shell(ctx, env, *global_id);
            cx.memo.insert(*id, obj);
            obj
        }
    }
}

fn deserialize_bigint<C: AsContextMut<Data = RuntimeState>>(ctx: &mut C, s: &str) -> i64 {
    let bi = BigInt::from_str(s).unwrap_or_else(|_| BigInt::from(0));
    let mut table = ctx
        .as_context()
        .data()
        .bigint_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(bi);
    value::encode_bigint_handle(handle)
}

fn deserialize_map<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    id: usize,
    entries: &[(SerializedValue, SerializedValue)],
    cx: &mut DeCtx,
) -> i64 {
    let new_handle = ctx.as_context().data().alloc_map_entry();
    let obj = create_map_shell_with_env(ctx, env, new_handle);
    cx.memo.insert(id, obj);
    let mut keys = Vec::with_capacity(entries.len());
    let mut values = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        keys.push(deserialize_one(ctx, env, k, cx));
        values.push(deserialize_one(ctx, env, v, cx));
    }
    {
        let mut table = ctx
            .as_context()
            .data()
            .map_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(new_handle as usize) {
            entry.keys = keys;
            entry.values = values;
        }
    }
    obj
}

fn deserialize_set<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    id: usize,
    values_src: &[SerializedValue],
    cx: &mut DeCtx,
) -> i64 {
    let new_handle = ctx.as_context().data().alloc_set_entry();
    let obj = create_set_shell_with_env(ctx, env, new_handle);
    cx.memo.insert(id, obj);
    let mut values = Vec::with_capacity(values_src.len());
    for v in values_src {
        values.push(deserialize_one(ctx, env, v, cx));
    }
    {
        let mut table = ctx
            .as_context()
            .data()
            .set_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(new_handle as usize) {
            entry.values = values;
        }
    }
    obj
}

fn create_map_shell_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    handle: u32,
) -> i64 {
    let obj = alloc_host_object(ctx, env, 2);
    ctx.as_context()
        .data()
        .bind_map_entry_owner(handle, value::decode_object_handle(obj));
    let size_fn = create_map_set_method(ctx.as_context().data(), MapSetMethodKind::Size);
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "__map_handle__",
        value::encode_f64(handle as f64),
    );
    let _ = define_host_accessor_property_with_env(
        ctx,
        env,
        obj,
        "size",
        size_fn,
        value::encode_undefined(),
    );
    obj
}

fn create_set_shell_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    handle: u32,
) -> i64 {
    let obj = alloc_host_object(ctx, env, 2);
    ctx.as_context()
        .data()
        .bind_set_entry_owner(handle, value::decode_object_handle(obj));
    let size_fn = create_map_set_method(ctx.as_context().data(), MapSetMethodKind::Size);
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "__set_handle__",
        value::encode_f64(handle as f64),
    );
    let _ = define_host_accessor_property_with_env(
        ctx,
        env,
        obj,
        "size",
        size_fn,
        value::encode_undefined(),
    );
    obj
}

fn create_date_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    ms: f64,
) -> i64 {
    let obj = alloc_host_object(ctx, env, 2);
    let get_time = create_date_method(ctx.as_context().data(), DateMethodKind::GetTime);
    let _ = define_host_data_property_with_env(ctx, env, obj, "__date_ms__", value::encode_f64(ms));
    let _ = define_host_data_property_with_env(ctx, env, obj, "getTime", get_time);
    obj
}

fn create_regexp_plain_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    source: &str,
    flags: &str,
) -> i64 {
    let obj = alloc_host_object(ctx, env, 2);
    let source_v = store_runtime_string_in_state(ctx.as_context().data(), source.to_string());
    let flags_v = store_runtime_string_in_state(ctx.as_context().data(), flags.to_string());
    let _ = define_host_data_property_with_env(ctx, env, obj, "source", source_v);
    let _ = define_host_data_property_with_env(ctx, env, obj, "flags", flags_v);
    obj
}

fn create_arraybuffer_from_bytes_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    bytes: Vec<u8>,
) -> i64 {
    let len = bytes.len() as u32;
    let ab_handle = {
        let mut table = ctx
            .as_context()
            .data()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(ArrayBufferEntry { data: bytes });
        handle
    };
    let obj = alloc_host_object(ctx, env, 2);
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "__arraybuffer_handle__",
        value::encode_f64(ab_handle as f64),
    );
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "byteLength",
        value::encode_f64(len as f64),
    );
    obj
}

fn materialize_sab_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    handle: u32,
) -> i64 {
    let Some(shared) = ctx.as_context().data().shared_state.clone() else {
        return value::encode_undefined();
    };
    let (byte_length, growable, max_byte_length) = {
        let table = shared.sab_table.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get(handle as usize) else {
            return value::encode_undefined();
        };
        (entry.byte_length, entry.growable(), entry.max_byte_length())
    };
    let obj = alloc_host_object(ctx, env, 4);
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        SAB_HANDLE_PROP,
        value::encode_f64(handle as f64),
    );
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "byteLength",
        value::encode_f64(byte_length as f64),
    );
    let _ =
        define_host_data_property_with_env(ctx, env, obj, "growable", value::encode_bool(growable));
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "maxByteLength",
        value::encode_f64(max_byte_length as f64),
    );
    obj
}

fn create_message_port_shell<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    global_id: u32,
) -> i64 {
    let obj = alloc_host_object(ctx, env, 1);
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        MESSAGE_PORT_ID_PROP,
        value::encode_f64(global_id as f64),
    );
    obj
}
