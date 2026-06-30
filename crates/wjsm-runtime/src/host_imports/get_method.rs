use crate::array_named_props::array_named_get_sync;
use crate::property_key::{is_symbol_name_id, name_id_to_property_key_value};
use crate::runtime_host_helpers::is_callable_in_runtime;
use crate::runtime_values::{find_property_slot_by_name_id, resolve_handle};
use crate::{WasmEnv, constants, value};
use wasmtime::{Caller, Extern};

use crate::RuntimeState;

/// GetMethod 的 symbol name_id 版本
pub(crate) fn get_method_by_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name_id: u32,
) -> Result<Option<i64>, i64> {
    // GetV(value, propertyKey)
    let func = get_v_by_name_id(caller, obj, name_id);

    // 如果是 undefined 或 null，返回 undefined
    if value::is_undefined(func) || value::is_null(func) {
        return Ok(None);
    }

    // 如果不可调用，抛出 TypeError
    if !is_callable_in_runtime(caller, func) {
        let msg_val = crate::runtime_render::store_runtime_string(
            caller,
            "method is not callable".to_string(),
        );
        let error_obj = crate::runtime_heap::create_error_object(
            caller,
            "TypeError",
            msg_val,
            value::encode_undefined(),
        );
        let mut errors = caller
            .data()
            .error_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let idx = errors.len() as u32;
        errors.push(crate::ErrorEntry {
            name: "TypeError".to_string(),
            message: "method is not callable".to_string(),
            value: error_obj,
        });
        return Err(value::encode_handle(value::TAG_EXCEPTION, idx));
    }

    Ok(Some(func))
}

/// ECMAScript `Get(O, P)`（支持 string 和 symbol name_id），供 `IsConcatSpreadable`、
/// Error.cause 提取等 builtin 路径使用。Proxy 路径需要 property key value，
/// string key 经 name_id_to_property_key_value 返回 None → 回退到 %Error.prototype% 等普通对象的原型链查找。
pub(crate) fn get_by_name_id_sync(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name_id: u32,
) -> i64 {
    if value::is_array(obj) && is_symbol_name_id(name_id) {
        let own = array_named_get_sync(caller, obj, name_id);
        if !value::is_undefined(own) {
            return own;
        }
    }

    if value::is_regexp(obj) {
        return crate::primitive_regexp_get_property_impl(caller, obj, name_id);
    }
    if value::is_proxy(obj) {
        let Some(prop) = name_id_to_property_key_value(name_id) else {
            return value::encode_undefined();
        };
        let rt = tokio::runtime::Handle::current();
        return tokio::task::block_in_place(|| {
            rt.block_on(
                crate::runtime_host_helpers::reflect_get_impl_with_receiver_async(
                    caller, obj, prop, obj,
                ),
            )
        });
    }
    if !value::is_js_object(obj) {
        return value::encode_undefined();
    }
    let Some(ptr) = resolve_handle(caller, obj) else {
        return value::encode_undefined();
    };
    let mut visited = std::collections::HashSet::new();
    get_by_name_id_on_proto_chain(caller, obj, ptr, name_id, &mut visited)
        .unwrap_or_else(value::encode_undefined)
}

fn read_getter_from_slot(caller: &mut Caller<'_, RuntimeState>, slot_offset: usize) -> i64 {
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return value::encode_undefined();
    };
    let data = memory.data(&*caller);
    if slot_offset + 24 > data.len() {
        return value::encode_undefined();
    }
    i64::from_le_bytes(data[slot_offset + 16..slot_offset + 24].try_into().unwrap())
}

fn invoke_getter_sync(caller: &mut Caller<'_, RuntimeState>, getter: i64, receiver: i64) -> i64 {
    if value::is_undefined(getter) || value::is_null(getter) {
        return value::encode_undefined();
    }
    if !value::is_callable(getter) {
        return value::encode_undefined();
    }
    if value::is_native_callable(getter) {
        return crate::call_native_callable_with_args_from_caller(caller, getter, receiver, vec![])
            .unwrap_or_else(value::encode_undefined);
    }
    let rt = tokio::runtime::Handle::current();
    tokio::task::block_in_place(|| {
        rt.block_on(crate::call_wasm_callback_async(
            caller,
            getter,
            receiver,
            &[],
        ))
    })
    .unwrap_or_else(|_| value::encode_undefined())
}

fn get_by_name_id_on_proto_chain(
    caller: &mut Caller<'_, RuntimeState>,
    receiver: i64,
    obj_ptr: usize,
    name_id: u32,
    visited: &mut std::collections::HashSet<usize>,
) -> Option<i64> {
    if !visited.insert(obj_ptr) {
        return None;
    }
    if let Some((slot_offset, flags, val)) = find_property_slot_by_name_id(caller, obj_ptr, name_id)
    {
        if (flags & constants::FLAG_IS_ACCESSOR) == 0 {
            return Some(val);
        }
        let getter = read_getter_from_slot(caller, slot_offset);
        return Some(invoke_getter_sync(caller, getter, receiver));
    }
    let env = WasmEnv::from_caller(caller)?;
    let proto_handle = {
        let data = env.memory.data(&*caller);
        if obj_ptr + 4 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr],
            data[obj_ptr + 1],
            data[obj_ptr + 2],
            data[obj_ptr + 3],
        ])
    };
    if proto_handle & 0x8000_0000 != 0 {
        // Proxy handle：高位标记表示这是 proxy_table 索引。
        // 重构 proxy 值并走 [[Get]] trap（经 reflect_get 实现）。
        let proxy_idx = (proto_handle & 0x7FFF_FFFF) as usize;
        let proxy_val = value::encode_proxy_handle(proxy_idx as u32);
        let prop = crate::property_key::name_id_to_property_key_value(name_id);
        if let Some(prop) = prop {
            let rt = tokio::runtime::Handle::current();
            return Some(
                tokio::task::block_in_place(|| {
                    rt.block_on(
                        crate::runtime_host_helpers::reflect_get_impl_with_receiver_async(
                            caller, proxy_val, prop, receiver,
                        ),
                    )
                }),
            );
        }
        return None;
    }
    let proto_ptr =
        crate::runtime_values::resolve_handle_idx_with_env(caller, &env, proto_handle as usize)?;
    get_by_name_id_on_proto_chain(caller, receiver, proto_ptr, name_id, visited)
}

/// GetV 的 symbol name_id 版本（不调用访问器；仅 GetMethod 等简单路径使用）
fn get_v_by_name_id(caller: &mut Caller<'_, RuntimeState>, value_val: i64, name_id: u32) -> i64 {
    if value::is_proxy(value_val) {
        return get_v_proxy_by_name_id(caller, value_val, name_id);
    }
    if value::is_regexp(value_val) {
        return crate::primitive_regexp_get_property_impl(caller, value_val, name_id);
    }

    let Some(ptr) = resolve_handle(caller, value_val) else {
        return value::encode_undefined();
    };

    read_object_property_by_name_id_proto_walk(caller, ptr, name_id)
        .unwrap_or_else(value::encode_undefined)
}

/// Proxy [[Get]] 的 name_id 版本（完整 trap；用于 GetMethod）
fn get_v_proxy_by_name_id(caller: &mut Caller<'_, RuntimeState>, proxy: i64, name_id: u32) -> i64 {
    let prop = match name_id_to_property_key_value(name_id) {
        Some(v) => v,
        None => return value::encode_undefined(),
    };
    let rt = tokio::runtime::Handle::current();
    tokio::task::block_in_place(|| {
        rt.block_on(
            crate::runtime_host_helpers::reflect_get_impl_with_receiver_async(
                caller, proxy, prop, proxy,
            ),
        )
    })
}

/// 沿原型链查找 symbol name_id 属性（数据属性槽值，不调用 getter）
fn read_object_property_by_name_id_proto_walk(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    name_id: u32,
) -> Option<i64> {
    let mut visited = std::collections::HashSet::new();
    read_object_property_by_name_id_proto_walk_impl(caller, obj_ptr, name_id, &mut visited)
}

fn read_object_property_by_name_id_proto_walk_impl(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    name_id: u32,
    visited: &mut std::collections::HashSet<usize>,
) -> Option<i64> {
    if !visited.insert(obj_ptr) {
        return None;
    }

    if let Some(val) =
        crate::runtime_values::read_object_property_by_name_id(caller, obj_ptr, name_id)
    {
        return Some(val);
    }

    let env = WasmEnv::from_caller(caller)?;
    let proto_handle = {
        let data = env.memory.data(&*caller);
        if obj_ptr + 4 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr],
            data[obj_ptr + 1],
            data[obj_ptr + 2],
            data[obj_ptr + 3],
        ])
    };

    if proto_handle == 0xFFFF_FFFF || proto_handle == 0 {
        return None;
    }

    let proto_ptr =
        crate::runtime_values::resolve_handle_idx_with_env(caller, &env, proto_handle as usize)?;
    read_object_property_by_name_id_proto_walk_impl(caller, proto_ptr, name_id, visited)
}
