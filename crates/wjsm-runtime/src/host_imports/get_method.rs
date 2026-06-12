use crate::runtime_values::{read_object_property_by_name_proto_walk, resolve_handle};
use crate::{value, WasmEnv};
use wasmtime::Caller;

use crate::RuntimeState;

/// GetMethod(value, propertyKey) 规范实现
/// 
/// 返回：
/// - Ok(Some(callable)) 如果找到可调用方法
/// - Ok(None) 如果方法是 undefined 或 null
/// - Err(exception_value) 如果方法存在但不可调用（TypeError）
pub(crate) fn get_method(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    prop_name: &str,
) -> Result<Option<i64>, i64> {
    // GetV(value, propertyKey)
    let func = get_v(caller, obj, prop_name);
    
    // 如果是 undefined 或 null，返回 undefined
    if value::is_undefined(func) || value::is_null(func) {
        return Ok(None);
    }
    
    // 如果不可调用，抛出 TypeError
    if !value::is_callable(func) {
        let msg_val = crate::runtime_render::store_runtime_string(caller, "method is not callable".to_string());
        let error_obj = crate::runtime_heap::create_error_object(caller, "TypeError", msg_val);
        let mut errors = caller.data().error_table.lock().expect("error table mutex");
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
    if !value::is_callable(func) {
        let msg_val = crate::runtime_render::store_runtime_string(caller, "method is not callable".to_string());
        let error_obj = crate::runtime_heap::create_error_object(caller, "TypeError", msg_val);
        let mut errors = caller.data().error_table.lock().expect("error table mutex");
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

/// GetV(value, propertyKey) 规范实现
/// 
/// 1. Let obj = ToObject(value).
/// 2. Return obj.[[Get]](propertyKey, value).
fn get_v(
    caller: &mut Caller<'_, RuntimeState>,
    value_val: i64,
    prop_name: &str,
) -> i64 {
    // 对于 Proxy，使用 Proxy [[Get]] trap
    if value::is_proxy(value_val) {
        return get_v_proxy(caller, value_val, prop_name);
    }
    
    // 对于普通对象，沿原型链查找
    let Some(ptr) = resolve_handle(caller, value_val) else {
        return value::encode_undefined();
    };
    
    let mut visited = std::collections::HashSet::new();
    read_object_property_by_name_proto_walk(caller, ptr, prop_name, &mut visited)
        .unwrap_or_else(value::encode_undefined)
}

/// GetV 的 symbol name_id 版本（支持原型链和 Proxy）
fn get_v_by_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    value_val: i64,
    name_id: u32,
) -> i64 {
    // 对于 Proxy，使用 Proxy [[Get]] trap
    if value::is_proxy(value_val) {
        return get_v_proxy_by_name_id(caller, value_val, name_id);
    }
    
    // 对于普通对象，沿原型链查找
    let Some(ptr) = resolve_handle(caller, value_val) else {
        return value::encode_undefined();
    };
    
    read_object_property_by_name_id_proto_walk(caller, ptr, name_id)
        .unwrap_or_else(value::encode_undefined)
}

/// Proxy [[Get]] 的字符串属性版本（同步路径，适用于 Symbol.iterator 等内置 symbol）
fn get_v_proxy(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    _prop_name: &str,
) -> i64 {
    // 简化实现：对于 Proxy，我们需要异步调用 trap
    // 但 GetMethod 在同步上下文中调用，所以这里暂时回退到目标对象
    // 完整实现需要将 async_iterator_from 的整个路径改为同步或重构
    
    // 获取 target（不触发 trap）
    let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
    let handle = value::decode_proxy_handle(proxy) as usize;
    if handle >= table.len() {
        return value::encode_undefined();
    }
    let entry = &table[handle];
    if entry.revoked {
        drop(table);
        return crate::runtime_heap::create_error_object(
            caller,
            "TypeError",
            value::encode_undefined(),
        );
    }
    let target = entry.target;
    drop(table);
    
    // 递归查找 target
    get_v(caller, target, _prop_name)
}

/// Proxy [[Get]] 的 name_id 版本
fn get_v_proxy_by_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    _name_id: u32,
) -> i64 {
    // 简化实现：直接查找 target
    let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
    let handle = value::decode_proxy_handle(proxy) as usize;
    if handle >= table.len() {
        return value::encode_undefined();
    }
    let entry = &table[handle];
    if entry.revoked {
        drop(table);
        return crate::runtime_heap::create_error_object(
            caller,
            "TypeError",
            value::encode_undefined(),
        );
    }
    let target = entry.target;
    drop(table);
    
    // 递归查找 target
    get_v_by_name_id(caller, target, _name_id)
}

/// 沿原型链查找 symbol name_id 属性
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
        return None; // 循环检测
    }
    
    // 在当前对象查找
    if let Some(val) = crate::runtime_values::read_object_property_by_name_id(caller, obj_ptr, name_id) {
        return Some(val);
    }
    
    // 沿原型链继续
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
    
    let proto_ptr = crate::runtime_values::resolve_handle_idx_with_env(caller, &env, proto_handle as usize)?;
    read_object_property_by_name_id_proto_walk_impl(caller, proto_ptr, name_id, visited)
}
