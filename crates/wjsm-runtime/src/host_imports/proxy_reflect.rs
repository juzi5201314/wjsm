use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Extern, Func, Linker};

use super::proxy_traps::proxy_trap_property_key_value;
use crate::*;

/// D4: 检查 proxy 是否已撤销，返回 Some(exception) 如果已撤销，否则 None。
pub(crate) fn check_proxy_revoked(
    caller: &mut Caller<'_, RuntimeState>,
    entry: &ProxyEntry,
    op: &str,
) -> Option<i64> {
    if entry.revoked {
        Some(make_type_error_exception(
            caller,
            &format!("Cannot perform '{}' on a proxy that has been revoked", op),
        ))
    } else {
        None
    }
}

fn read_object_proto_ptr(caller: &mut Caller<'_, RuntimeState>, obj_ptr: usize) -> Option<usize> {
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
    resolve_handle_idx_with_env(caller, &env, proto_handle as usize)
}

fn read_setter_from_slot(caller: &mut Caller<'_, RuntimeState>, slot_offset: usize) -> i64 {
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return value::encode_undefined();
    };
    let data = memory.data(&*caller);
    if slot_offset + 32 > data.len() {
        return value::encode_undefined();
    }
    i64::from_le_bytes(data[slot_offset + 24..slot_offset + 32].try_into().unwrap())
}

async fn invoke_property_setter(
    caller: &mut Caller<'_, RuntimeState>,
    setter: i64,
    receiver: i64,
    val: i64,
) -> bool {
    if value::is_undefined(setter) || value::is_null(setter) {
        return false;
    }
    if !value::is_callable(setter) {
        return false;
    }
    // §10.1.9.2：调用 setter 后返回 true（忽略 setter 自身返回值）；仅当 setter 抛异常时视为失败。
    match call_wasm_callback_async(caller, setter, receiver, &[val]).await {
        Ok(r) => !value::is_exception(r),
        Err(_) => false,
    }
}

fn receiver_own_descriptor_from_trap_result(
    caller: &mut Caller<'_, RuntimeState>,
    desc_handle: i64,
) -> Option<PropertyDescriptor> {
    if value::is_undefined(desc_handle) {
        return None;
    }
    parse_descriptor(caller, desc_handle).ok()
}

fn make_data_descriptor_object_for_define(caller: &mut Caller<'_, RuntimeState>, val: i64) -> i64 {
    let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let desc = alloc_host_object(caller, &_wjsm_env, 4);
    let _ = define_host_data_property_from_caller(caller, desc, "value", val);
    let _ =
        define_host_data_property_from_caller(caller, desc, "writable", value::encode_bool(true));
    let _ =
        define_host_data_property_from_caller(caller, desc, "enumerable", value::encode_bool(true));
    let _ = define_host_data_property_from_caller(
        caller,
        desc,
        "configurable",
        value::encode_bool(true),
    );
    desc
}

/// §10.1.9.2: define {value:V} on Receiver (Proxy traps or ordinary CreateDataProperty).
pub(crate) async fn define_value_on_receiver(
    caller: &mut Caller<'_, RuntimeState>,
    receiver: i64,
    name_id: u32,
    val: i64,
) -> bool {
    if !value::is_object(receiver)
        && !value::is_function(receiver)
        && !value::is_array(receiver)
        && !value::is_proxy(receiver)
    {
        return false;
    }

    let prop_key = proxy_trap_property_key_value(caller, name_id as i32);
    let existing_handle =
        reflect_get_own_property_descriptor_on_object_async(caller, receiver, prop_key).await;
    if value::is_exception(existing_handle) {
        return false;
    }

    let existing = receiver_own_descriptor_from_trap_result(caller, existing_handle);
    if let Some(desc) = &existing {
        let completed = complete_property_descriptor(desc.clone());
        if is_accessor_descriptor(&completed) {
            return false;
        }
        if completed.writable == Some(false) {
            return false;
        }
        let desc_obj = make_data_descriptor_object_for_define(caller, val);
        return define_property_internal_async(caller, receiver, prop_key, desc_obj)
            .await
            .unwrap_or_default();
    }

    if !is_extensible_impl(caller, receiver) {
        return false;
    }

    let desc_obj = make_data_descriptor_object_for_define(caller, val);
    define_property_internal_async(caller, receiver, prop_key, desc_obj)
        .await
        .unwrap_or_default()
}

/// ECMAScript OrdinarySet / OrdinarySetWithOwnDescriptor (§10.1.9, §10.1.9.2).
pub(crate) async fn ordinary_set_by_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    receiver: i64,
    name_id: u32,
    val: i64,
) -> bool {
    #[cfg(feature = "managed-heap-v2")]
    {
        let start_handle = handle_index_of(caller, obj) as u32;
        let access = caller.data().heap_access_v2().clone();
        if access.resolve_handle(start_handle).is_ok() {
            let Some(key) = crate::property_key::canonicalize_v2_name_id(caller, name_id) else {
                return false;
            };
            let mut current = start_handle;
            let mut visited = std::collections::HashSet::new();
            loop {
                if !visited.insert(current) {
                    return false;
                }
                if let Ok(Some(property)) = access.get_property_slot(current, key) {
                    if property.flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
                        return invoke_property_setter(
                            caller,
                            property.setter as i64,
                            receiver,
                            val,
                        )
                        .await;
                    }
                    if property.flags & constants::FLAG_WRITABLE as u32 == 0 {
                        return false;
                    }
                    let current_val = value::encode_object_handle(current);
                    if current_val == receiver
                        || (value::is_function(receiver)
                            && handle_index_of(caller, receiver) as u32 == current)
                    {
                        return access
                            .set_property(current, key, val as u64)
                            .is_ok();
                    }
                    return define_value_on_receiver(caller, receiver, name_id, val).await;
                }
                let Ok(proto) = access.prototype(current) else {
                    return define_value_on_receiver(caller, receiver, name_id, val).await;
                };
                if proto == 0xFFFF_FFFF || proto == current {
                    return define_value_on_receiver(caller, receiver, name_id, val).await;
                }
                if access.resolve_handle(proto).is_err() {
                    return define_value_on_receiver(caller, receiver, name_id, val).await;
                }
                current = proto;
            }
        }
    }

    let mut current = obj;
    let mut visited = std::collections::HashSet::new();

    loop {
        let Some(current_ptr) = resolve_handle(caller, current) else {
            return false;
        };
        if !visited.insert(current_ptr) {
            return false;
        }

        if let Some((slot_offset, flags, _)) =
            find_property_slot_by_name_id(caller, current_ptr, name_id)
        {
            let is_accessor = (flags & constants::FLAG_IS_ACCESSOR) != 0;
            if is_accessor {
                // V2 find_property 可能返回哨兵偏移；禁止读 memory32。
                if slot_offset == usize::MAX {
                    return false;
                }
                let setter = read_setter_from_slot(caller, slot_offset);
                return invoke_property_setter(caller, setter, receiver, val).await;
            }
            if (flags & constants::FLAG_WRITABLE) == 0 {
                return false;
            }
            if current == receiver {
                write_object_property_by_name_id(caller, current_ptr, current, name_id, val, flags);
                return true;
            }
            return define_value_on_receiver(caller, receiver, name_id, val).await;
        }

        let Some(proto_ptr) = read_object_proto_ptr(caller, current_ptr) else {
            return define_value_on_receiver(caller, receiver, name_id, val).await;
        };

        if let Some((slot_offset, parent_flags, _)) =
            find_property_slot_by_name_id(caller, proto_ptr, name_id)
        {
            let parent_accessor = (parent_flags & constants::FLAG_IS_ACCESSOR) != 0;
            if parent_accessor {
                if slot_offset == usize::MAX {
                    return false;
                }
                let setter = read_setter_from_slot(caller, slot_offset);
                return invoke_property_setter(caller, setter, receiver, val).await;
            }
            if (parent_flags & constants::FLAG_WRITABLE) == 0 {
                return false;
            }
            return define_value_on_receiver(caller, receiver, name_id, val).await;
        }

        let proto_handle = {
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            let data = env.memory.data(&*caller);
            if current_ptr + 4 > data.len() {
                return define_value_on_receiver(caller, receiver, name_id, val).await;
            }
            u32::from_le_bytes(data[current_ptr..current_ptr + 4].try_into().unwrap())
        };
        current = value::encode_object_handle(proto_handle);
    }
}

fn has_property_by_name_id_proto_walk(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    name_id: u32,
    visited: &mut std::collections::HashSet<usize>,
) -> bool {
    if !visited.insert(obj_ptr) {
        return false;
    }
    if find_property_slot_by_name_id(caller, obj_ptr, name_id).is_some() {
        return true;
    }
    let Some(proto_ptr) = read_object_proto_ptr(caller, obj_ptr) else {
        return false;
    };
    has_property_by_name_id_proto_walk(caller, proto_ptr, name_id, visited)
}

pub(crate) async fn reflect_set_impl_with_receiver(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
    val: i64,
    receiver: i64,
) -> i64 {
    if !value::is_js_object(target) && !value::is_array(target) && !value::is_function(target) {
        return value::encode_bool(false);
    }
    let Some(name_id) = property_key_value_to_name_id(caller, prop, true) else {
        return value::encode_bool(false);
    };
    let ok = ordinary_set_by_name_id(caller, target, receiver, name_id, val).await;
    value::encode_bool(ok)
}

/// Object.assign (§20.1.2.1): Set(target, key, value, true) for each enumerable own key.
pub(crate) async fn object_assign_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    if !value::is_object(target) && !value::is_function(target) && !value::is_array(target) {
        set_runtime_error(
            caller.data(),
            "TypeError: target is not an object".to_string(),
        );
        return value::encode_undefined();
    }
    for i in 0..args_count {
        let mut source_val = read_shadow_arg(caller, args_base, i as u32);
        if value::is_undefined(source_val) || value::is_null(source_val) {
            continue;
        }
        if !value::is_js_object(source_val) {
            source_val = to_object(caller, source_val);
        }
        if resolve_handle(caller, source_val).is_none() {
            continue;
        }
        let names = collect_own_property_names_from_value(caller, source_val, true);
        for name in names {
            let name_val = store_runtime_string(caller, name.clone());
            let prop_val =
                reflect_get_impl_with_receiver_async(caller, source_val, name_val, source_val)
                    .await;
            // 优先 memory c-string（与编译期 name_id 同形）；否则 intern runtime key。
            let name_id = match find_memory_c_string(caller, &name) {
                Some(id) => id,
                None => crate::property_key::encode_runtime_string_name_id(
                    crate::property_key::intern_runtime_property_key(
                        caller.data(),
                        crate::runtime_string::RuntimeString::from_utf8_str(&name),
                    ),
                ),
            };
            if !ordinary_set_by_name_id(caller, target, target, name_id, prop_val).await {
                return make_type_error_exception(caller, "Cannot assign to read only property");
            }
        }
    }
    target
}

pub(crate) fn reflect_has_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
) -> i64 {
    if !value::is_js_object(target) && !value::is_array(target) && !value::is_function(target) {
        return value::encode_bool(false);
    }
    let Some(ptr) = resolve_handle(caller, target) else {
        return value::encode_bool(false);
    };
    let Some(name_id) = property_key_value_to_name_id(caller, prop, false) else {
        return value::encode_bool(false);
    };
    let mut visited = std::collections::HashSet::new();
    let found = has_property_by_name_id_proto_walk(caller, ptr, name_id, &mut visited);
    value::encode_bool(found)
}

pub(crate) fn reflect_delete_property_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
) -> i64 {
    let prop_name = if value::is_string(prop) {
        get_string_utf8_lossy(caller, prop)
    } else {
        match render_value(caller, prop) {
            Ok(name) => name,
            Err(_) => return value::encode_bool(true),
        }
    };
    // 数组元素删除必须在 name_id 解析前完成：数字索引可能尚未 intern 到
    // memory c-string 表，property_key_value_to_name_id(..., false) 会失败并
    // 误返回 true，导致 hole 从未写入。
    if value::is_array(target)
        && let Ok(index) = prop_name.parse::<u32>()
    {
        #[cfg(feature = "managed-heap-v2")]
        {
            let handle = value::decode_handle(target);
            if caller
                .data()
                .heap_access_v2()
                .resolve_handle(handle)
                .is_ok()
            {
                if let Err(error) = crate::set_v2_array_element(
                    caller,
                    handle,
                    index,
                    value::encode_array_hole() as u64,
                ) {
                    set_runtime_error(caller.data(), format!("V2 array delete hole: {error}"));
                    return value::encode_bool(false);
                }
                return value::encode_bool(true);
            }
        }
        if let Some(ptr) = resolve_handle(caller, target) {
            crate::runtime_values::write_array_hole(caller, ptr, index);
        }
        return value::encode_bool(true);
    }
    let Some(ptr) = resolve_handle(caller, target) else {
        return value::encode_bool(true);
    };
    let Some(name_id) = property_key_value_to_name_id(caller, prop, false) else {
        return value::encode_bool(true);
    };
    let _ = ptr;
    delete_property_by_name_id(caller, target, name_id)
}

pub(crate) fn delete_property_by_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: u32,
) -> i64 {
    #[cfg(feature = "managed-heap-v2")]
    {
        let handle = handle_index_of(caller, target) as u32;
        let access = caller.data().heap_access_v2().clone();
        if access.resolve_handle(handle).is_ok() {
            let Some(key) = crate::property_key::canonicalize_v2_name_id(caller, name_id) else {
                return value::encode_bool(true);
            };
            return match access.get_property_slot(handle, key) {
                Ok(Some(property)) => {
                    if property.flags & constants::FLAG_CONFIGURABLE as u32 == 0 {
                        value::encode_bool(false)
                    } else {
                        match access.delete_property(handle, key) {
                            Ok(deleted) => value::encode_bool(deleted),
                            Err(_) => value::encode_bool(false),
                        }
                    }
                }
                Ok(None) => value::encode_bool(true),
                Err(_) => value::encode_bool(false),
            };
        }
    }
    let Some(ptr) = resolve_handle(caller, target) else {
        return value::encode_bool(true);
    };
    let Some((slot_offset, flags, _val)) = find_property_slot_by_name_id(caller, ptr, name_id)
    else {
        return value::encode_bool(true);
    };
    if (flags & constants::FLAG_CONFIGURABLE) == 0 {
        return value::encode_bool(false);
    }
    let Some(env) = WasmEnv::from_caller(caller) else {
        return value::encode_bool(false);
    };
    let data = env.memory.data(&*caller);
    if ptr + 16 > data.len() || slot_offset + 32 > data.len() {
        return value::encode_bool(false);
    }
    let num_props = u32::from_le_bytes([
        data[ptr + 12],
        data[ptr + 13],
        data[ptr + 14],
        data[ptr + 15],
    ]) as usize;
    if num_props == 0 {
        return value::encode_bool(true);
    }
    {
        let data = env.memory.data_mut(&mut *caller);
        data[ptr + 12..ptr + 16].copy_from_slice(&(num_props as u32 - 1).to_le_bytes());
    }
    let target_handle = handle_index_of(caller, target) as u32;
    let prop_idx = (slot_offset - (ptr + 16)) / 32;
    for i in prop_idx..num_props - 1 {
        let src = ptr + 16 + (i + 1) * 32;
        let dst = ptr + 16 + i * 32;
        let (name, shifted_flags, val, getter, setter) = {
            let data = env.memory.data(&*caller);
            if src + 32 > data.len() || dst + 32 > data.len() {
                break;
            }
            (
                u32::from_le_bytes(data[src..src + 4].try_into().unwrap()),
                i32::from_le_bytes(data[src + 4..src + 8].try_into().unwrap()),
                i64::from_le_bytes(data[src + 8..src + 16].try_into().unwrap()),
                i64::from_le_bytes(data[src + 16..src + 24].try_into().unwrap()),
                i64::from_le_bytes(data[src + 24..src + 32].try_into().unwrap()),
            )
        };
        {
            let data = env.memory.data_mut(&mut *caller);
            data[dst..dst + 4].copy_from_slice(&name.to_le_bytes());
            data[dst + 4..dst + 8].copy_from_slice(&shifted_flags.to_le_bytes());
        }
        let _ = crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            target_handle,
            i,
            crate::runtime_gc::heap_access::SlotPart::Value,
            val,
        );
        let _ = crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            target_handle,
            i,
            crate::runtime_gc::heap_access::SlotPart::Getter,
            getter,
        );
        let _ = crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            target_handle,
            i,
            crate::runtime_gc::heap_access::SlotPart::Setter,
            setter,
        );
    }
    value::encode_bool(true)
}

pub(crate) async fn extract_array_like_elements(
    caller: &mut Caller<'_, RuntimeState>,
    arr_like: i64,
) -> Result<Vec<i64>, String> {
    let mut elements = Vec::new();
    if value::is_array(arr_like) {
        #[cfg(feature = "managed-heap-v2")]
        {
            let handle = value::decode_array_handle(arr_like);
            let access = caller.data().heap_access_v2();
            if access.resolve_handle(handle).is_ok() {
                let len = access
                    .array_length(handle)
                    .map_err(|error| error.to_string())?;
                for index in 0..len {
                    elements.push(
                        access
                            .get_element(handle, index)
                            .map_err(|error| error.to_string())?
                            .map(|element| element as i64)
                            .unwrap_or_else(value::encode_undefined),
                    );
                }
                return Ok(elements);
            }
        }
        let handle = value::decode_array_handle(arr_like) as usize;
        let Some(ptr) = resolve_handle_idx(caller, handle) else {
            return Ok(elements);
        };
        let len = {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return Ok(elements);
            };
            let data = memory.data(&*caller);
            if ptr + 12 > data.len() {
                return Ok(elements);
            }
            u32::from_le_bytes([data[ptr + 8], data[ptr + 9], data[ptr + 10], data[ptr + 11]])
                as usize
        };
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return Ok(elements);
        };
        for i in 0..len {
            let mut buf = [0u8; 8];
            if memory
                .read(&mut *caller, ptr + 16 + i * 8, &mut buf)
                .is_ok()
            {
                elements.push(i64::from_le_bytes(buf));
            }
        }
    } else if value::is_object(arr_like) || value::is_proxy(arr_like) {
        let len_prop = store_runtime_string(caller, "length".to_string());
        let len_val =
            reflect_get_impl_with_receiver_async(caller, arr_like, len_prop, arr_like).await;
        let len = if value::is_f64(len_val) {
            value::decode_f64(len_val) as usize
        } else {
            0
        };
        for i in 0..len {
            let idx_prop = value::encode_f64(i as f64);
            let val =
                reflect_get_impl_with_receiver_async(caller, arr_like, idx_prop, arr_like).await;
            elements.push(val);
        }
    }
    Ok(elements)
}

pub(crate) async fn reflect_apply_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    this_arg: i64,
    args: &[i64],
) -> i64 {
    let Some(env) = WasmEnv::from_caller(caller) else {
        return value::encode_undefined();
    };
    let Some(saved_sp) = push_args_to_shadow_stack(caller, &env, args) else {
        return value::encode_undefined();
    };
    let result =
        resolve_and_call_async(caller, target, this_arg, saved_sp, args.len() as i32).await;
    restore_shadow_sp(caller, &env, saved_sp);
    result
}

pub(crate) async fn reflect_construct_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    args: &[i64],
    new_target: i64,
) -> i64 {
    let this_obj = {
        let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &_wjsm_env, 4)
    };
    let proto_prop = store_runtime_string(caller, "prototype".to_string());
    let proto_val =
        reflect_get_impl_with_receiver_async(caller, new_target, proto_prop, new_target).await;
    if value::is_object(proto_val)
        || value::is_array(proto_val)
        || value::is_proxy(proto_val)
        || value::is_null(proto_val)
    {
        let _ = reflect_set_prototype_of_fn_impl(caller, this_obj, proto_val).await;
    }

    let Some(env) = WasmEnv::from_caller(caller) else {
        return this_obj;
    };
    let Some(saved_sp) = push_args_to_shadow_stack(caller, &env, args) else {
        return this_obj;
    };
    let result =
        resolve_and_call_async(caller, target, this_obj, saved_sp, args.len() as i32).await;
    restore_shadow_sp(caller, &env, saved_sp);

    if value::is_js_object(result) {
        result
    } else {
        this_obj
    }
}

pub(crate) async fn reflect_get_prototype_of_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
) -> i64 {
    if !value::is_object(target)
        && !value::is_array(target)
        && !value::is_function(target)
        && !value::is_proxy(target)
        && !value::is_regexp(target)
    {
        set_runtime_error(
            caller.data(),
            "TypeError: Reflect.getPrototypeOf called on non-object".to_string(),
        );
        return value::encode_undefined();
    }
    proxy_or_target_get_prototype_of_impl_async(caller, target).await
}

pub(crate) async fn reflect_get_prototype_of_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
) -> i64 {
    if value::is_proxy(target) {
        let handle = value::decode_proxy_handle(target) as usize;
        let entry = {
            let table = caller
                .data()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if let Some(exc) = check_proxy_revoked(caller, &entry, "getPrototypeOf") {
                return exc;
            }
            if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                let trap = read_object_property_by_name(caller, handler_ptr, "getPrototypeOf")
                    .unwrap_or_else(value::encode_undefined);
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    let res =
                        call_wasm_callback_async(caller, trap, entry.handler, &[entry.target])
                            .await
                            .unwrap_or_else(|_| value::encode_null());
                    // 不变量检查: getPrototypeOf trap 返回值必须是 null 或对象
                    if !value::is_null(res)
                        && !value::is_object(res)
                        && !value::is_array(res)
                        && !value::is_proxy(res)
                        && !value::is_function(res)
                    {
                        set_runtime_error(
                            caller.data(),
                            "TypeError: Proxy getPrototypeOf must return an object or null"
                                .to_string(),
                        );
                        return value::encode_null();
                    }
                    // 不变量检查: 若 target 不可扩展，返回的原型必须与 target 原型一致
                    let ext = is_extensible_impl(caller, entry.target);
                    if !ext {
                        let target_proto =
                            Box::pin(reflect_get_prototype_of_impl(caller, entry.target)).await;
                        if res != target_proto {
                            set_runtime_error(caller.data(), "TypeError: Proxy getPrototypeOf invariant violated: target is not extensible and trap returned different prototype".to_string());
                            return value::encode_null();
                        }
                    }
                    return res;
                }
            }
            return Box::pin(reflect_get_prototype_of_impl(caller, entry.target)).await;
        }
    }
    // TAG_REGEXP 无 obj_table 条目，其 [[Prototype]] 是 RegExp.prototype 对象，
    // 不能走 resolve_handle（会得到 null）；与 ordinary_has_instance_async 同构。
    if value::is_regexp(target) {
        if !value::is_object(caller.data().regexp_prototype)
            && let Some(env) = WasmEnv::from_caller(caller)
        {
            crate::runtime_heap::ensure_regexp_prototype_initialized(caller, &env);
        }
        let proto = caller.data().regexp_prototype;
        return if value::is_object(proto) {
            proto
        } else {
            value::encode_null()
        };
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

async fn is_prototype_circular_chain(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    proto: i64,
) -> bool {
    let mut current = proto;
    let mut visited = std::collections::HashSet::new();
    while !value::is_null(current) && !value::is_undefined(current) {
        if current == target {
            return true;
        }
        if value::is_js_object(current) {
            let handle = handle_index_of(caller, current) as u32;
            if !visited.insert(handle) {
                break;
            }
        }
        current = reflect_get_prototype_of_impl(caller, current).await;
    }
    false
}

pub(crate) async fn reflect_set_prototype_of_fn_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    proto: i64,
) -> i64 {
    if !is_extensible_impl(caller, target) {
        let current_proto = reflect_get_prototype_of_impl(caller, target).await;
        return value::encode_bool(current_proto == proto);
    }
    if is_prototype_circular_chain(caller, target, proto).await {
        return value::encode_bool(false);
    }
    let proto_handle = if value::is_null(proto) {
        0xFFFF_FFFF
    } else if value::is_object(proto) {
        value::decode_object_handle(proto)
    } else if value::is_array(proto) {
        value::decode_array_handle(proto)
    } else if value::is_proxy(proto) {
        value::decode_proxy_handle(proto) | 0x8000_0000
    } else if value::is_function(proto) {
        let func_idx = value::decode_function_idx(proto);
        let base = caller
            .get_export("__function_props_base")
            .and_then(|e| e.into_global())
            .and_then(|g| g.get(&mut *caller).i32())
            .unwrap_or(0) as u32;
        base + func_idx
    } else if value::is_closure(proto) {
        let closure_idx = value::decode_closure_idx(proto) as usize;
        let func_idx = caller
            .data()
            .closures
            .lock()
            .ok()
            .and_then(|g| g.get(closure_idx).map(|e| e.func_idx))
            .unwrap_or(0);
        let base = caller
            .get_export("__function_props_base")
            .and_then(|e| e.into_global())
            .and_then(|g| g.get(&mut *caller).i32())
            .unwrap_or(0) as u32;
        base + func_idx
    } else {
        0xFFFF_FFFF
    };
    let Some(env) = WasmEnv::from_caller(caller) else {
        return value::encode_bool(false);
    };
    let target_handle = handle_index_of(caller, target) as u32;
    value::encode_bool(
        crate::runtime_gc::heap_access::write_proto(caller, &env, target_handle, proto_handle)
            .is_some(),
    )
}

fn alloc_data_property_descriptor(
    caller: &mut Caller<'_, RuntimeState>,
    value_val: i64,
    writable: bool,
    enumerable: bool,
    configurable: bool,
) -> i64 {
    let desc = {
        let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &_wjsm_env, 4)
    };
    let _ = define_host_data_property_from_caller(caller, desc, "value", value_val);
    let _ = define_host_data_property_from_caller(
        caller,
        desc,
        "writable",
        value::encode_bool(writable),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        desc,
        "enumerable",
        value::encode_bool(enumerable),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        desc,
        "configurable",
        value::encode_bool(configurable),
    );
    desc
}

pub(crate) fn reflect_get_own_property_descriptor_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
) -> i64 {
    let Some(ptr) = resolve_handle(caller, target) else {
        return value::encode_undefined();
    };
    let name_id = if let Some(name_id) = symbol_value_to_name_id(prop) {
        name_id
    } else {
        let prop_name = match render_value(caller, prop) {
            Ok(name) => name,
            Err(_) => return value::encode_undefined(),
        };
        let Some(name_id) = find_memory_c_string(caller, &prop_name) else {
            return value::encode_undefined();
        };
        name_id
    };
    if value::is_array(target) {
        if !crate::property_key::is_symbol_name_id(name_id) {
            let name_bytes = crate::runtime_render::read_string_bytes(caller, name_id);
            if name_bytes == b"length" {
                let len = read_array_length(caller, ptr).unwrap_or(0);
                return alloc_data_property_descriptor(
                    caller,
                    value::encode_f64(len as f64),
                    true,
                    false,
                    false,
                );
            }
        }
        if let Some(slot) =
            crate::array_named_props::ArrayNamedPropsStore::get_slot(caller, target, name_id)
        {
            return alloc_data_property_descriptor(
                caller,
                slot.value,
                (slot.flags & constants::FLAG_WRITABLE) != 0,
                (slot.flags & constants::FLAG_ENUMERABLE) != 0,
                (slot.flags & constants::FLAG_CONFIGURABLE) != 0,
            );
        }
    }

    let Some((slot_offset, flags, val)) = find_property_slot_by_name_id(caller, ptr, name_id)
    else {
        return value::encode_undefined();
    };
    let is_accessor = (flags & constants::FLAG_IS_ACCESSOR) != 0;
    let enumerable = (flags & constants::FLAG_ENUMERABLE) != 0;
    let configurable = (flags & constants::FLAG_CONFIGURABLE) != 0;
    let (getter_val, setter_val) = if is_accessor {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return value::encode_undefined();
        };
        let data = memory.data(&*caller);
        if slot_offset + 32 > data.len() {
            return value::encode_undefined();
        }
        let g = i64::from_le_bytes([
            data[slot_offset + 16],
            data[slot_offset + 17],
            data[slot_offset + 18],
            data[slot_offset + 19],
            data[slot_offset + 20],
            data[slot_offset + 21],
            data[slot_offset + 22],
            data[slot_offset + 23],
        ]);
        let s = i64::from_le_bytes([
            data[slot_offset + 24],
            data[slot_offset + 25],
            data[slot_offset + 26],
            data[slot_offset + 27],
            data[slot_offset + 28],
            data[slot_offset + 29],
            data[slot_offset + 30],
            data[slot_offset + 31],
        ]);
        (g, s)
    } else {
        (value::encode_undefined(), value::encode_undefined())
    };
    let desc = {
        let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &_wjsm_env, 4)
    };
    if is_accessor {
        let _ = define_host_data_property_from_caller(caller, desc, "get", getter_val);
        let _ = define_host_data_property_from_caller(caller, desc, "set", setter_val);
    } else {
        let _ = define_host_data_property_from_caller(caller, desc, "value", val);
        let _ = define_host_data_property_from_caller(
            caller,
            desc,
            "writable",
            value::encode_bool((flags & constants::FLAG_WRITABLE) != 0),
        );
    }
    let _ = define_host_data_property_from_caller(
        caller,
        desc,
        "enumerable",
        value::encode_bool(enumerable),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        desc,
        "configurable",
        value::encode_bool(configurable),
    );
    desc
}

pub(crate) fn reflect_own_keys_impl(caller: &mut Caller<'_, RuntimeState>, target: i64) -> i64 {
    let Some(ptr) = resolve_handle(caller, target) else {
        return value::encode_undefined();
    };
    let keys = collect_own_property_key_values(caller, ptr, false);
    let len = keys.len() as u32;
    let arr = alloc_array(caller, len);
    for (i, key) in keys.into_iter().enumerate() {
        set_array_elem(caller, arr, i as i32, key);
    }
    if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
        write_array_length(caller, arr_ptr, len);
    }
    arr
}

/// Proxy ownKeys 陷阱：返回陷阱结果数组，失败或应回退时返回 undefined。
pub(crate) async fn proxy_own_keys_trap_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
) -> i64 {
    if !value::is_proxy(target) {
        return value::encode_undefined();
    }
    let handle = value::decode_proxy_handle(target) as usize;
    let entry = {
        let table = caller
            .data()
            .proxy_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table.get(handle).cloned()
    };
    let Some(entry) = entry else {
        return value::encode_undefined();
    };
    if let Some(exc) = check_proxy_revoked(caller, &entry, "ownKeys") {
        return exc;
    }
    #[cfg(feature = "managed-heap-v2")]
    let trap = read_host_data_property_v2(caller, entry.handler, "ownKeys")
        .unwrap_or_else(value::encode_undefined);
    #[cfg(not(feature = "managed-heap-v2"))]
    let trap = resolve_handle(caller, entry.handler)
        .and_then(|handler_ptr| read_object_property_by_name(caller, handler_ptr, "ownKeys"))
        .unwrap_or_else(value::encode_undefined);
    if value::is_undefined(trap) || value::is_null(trap) {
        return reflect_own_keys_impl(caller, entry.target);
    }
    let trap_res = call_wasm_callback_async(caller, trap, entry.handler, &[entry.target]).await;
    let keys_val = match trap_res {
        Ok(res) => res,
        Err(e) => {
            return make_type_error_exception(caller, &format!("Proxy ownKeys trap failed: {}", e));
        }
    };
    let keys = match extract_array_like_elements(caller, keys_val).await {
        Ok(arr) => arr,
        Err(err) => {
            return make_type_error_exception(caller, &err);
        }
    };
    let ext = is_extensible_impl(caller, entry.target);
    #[cfg(feature = "managed-heap-v2")]
    if caller
        .data()
        .heap_access_v2()
        .resolve_handle(value::decode_handle(entry.target))
        .is_ok()
    {
        let mut trap_keys_str = Vec::new();
        let mut trap_keys_sym = Vec::new();
        for key in &keys {
            if value::is_symbol(*key) {
                trap_keys_sym.push(*key);
            } else if let Ok(key) = render_value(caller, *key) {
                trap_keys_str.push(key);
            }
        }
        let target_slots = caller
            .data()
            .heap_access_v2()
            .own_property_slots(value::decode_handle(entry.target))
            .unwrap_or_default();
        let mut target_strings = Vec::new();
        let mut target_symbols = Vec::new();
        for (name_id, flags) in target_slots {
            let configurable = flags & constants::FLAG_CONFIGURABLE as u32 != 0;
            match crate::property_key::decode_name_id(name_id) {
                crate::property_key::DecodedNameId::RuntimeString(index) => {
                    if let Some(key) =
                        crate::property_key::runtime_property_key_units(caller.data(), index)
                    {
                        target_strings.push((key.to_utf8_lossy(), configurable));
                    }
                }
                crate::property_key::DecodedNameId::Symbol(index) => {
                    target_symbols.push((value::encode_symbol_handle(index), configurable));
                }
                crate::property_key::DecodedNameId::MemoryString(index) => {
                    if let Ok(key) = render_value(caller, value::encode_string_ptr(index)) {
                        target_strings.push((key, configurable));
                    }
                }
            }
        }
        let violates_invariant = if !ext {
            target_strings.len() != trap_keys_str.len()
                || target_symbols.len() != trap_keys_sym.len()
                || target_strings
                    .iter()
                    .any(|(key, _)| !trap_keys_str.contains(key))
                || target_symbols.iter().any(|(key, _)| {
                    !trap_keys_sym
                        .iter()
                        .any(|trap_key| same_value_zero(caller, *trap_key, *key))
                })
        } else {
            target_strings
                .iter()
                .any(|(key, configurable)| !*configurable && !trap_keys_str.contains(key))
                || target_symbols.iter().any(|(key, configurable)| {
                    !*configurable
                        && !trap_keys_sym
                            .iter()
                            .any(|trap_key| same_value_zero(caller, *trap_key, *key))
                })
        };
        if violates_invariant {
            return make_type_error_exception(
                caller,
                "Proxy ownKeys invariant violated for V2 target",
            );
        }
        let arr = alloc_array(caller, keys.len() as u32);
        for (index, key) in keys.into_iter().enumerate() {
            set_array_elem(caller, arr, index as i32, key);
        }
        return arr;
    }
    let Some(t_ptr) = resolve_handle(caller, entry.target) else {
        return value::encode_undefined();
    };
    let target_keys_str = collect_own_property_names(caller, t_ptr, false);
    let target_keys_sym: Vec<i64> = collect_own_property_key_values(caller, t_ptr, true);
    let mut trap_keys_str = Vec::new();
    let mut trap_keys_sym = Vec::new();
    for &k in &keys {
        if value::is_symbol(k) {
            trap_keys_sym.push(k);
        } else if let Ok(k_str) = render_value(caller, k) {
            trap_keys_str.push(k_str);
        }
    }
    if !ext {
        let mut match_all = true;
        for tk in &target_keys_str {
            if !trap_keys_str.contains(tk) {
                match_all = false;
                break;
            }
        }
        if match_all {
            for &tk in &target_keys_sym {
                if !trap_keys_sym
                    .iter()
                    .any(|&s| same_value_zero(caller, s, tk))
                {
                    match_all = false;
                    break;
                }
            }
        }
        if !match_all
            || trap_keys_str.len() != target_keys_str.len()
            || trap_keys_sym.len() != target_keys_sym.len()
        {
            return make_type_error_exception(
                caller,
                "Proxy ownKeys invariant violated: target is non-extensible and keys do not match target keys",
            );
        }
    } else {
        for tk in &target_keys_str {
            if let Some(tk_c) = find_memory_c_string(caller, tk)
                && let Some((_, flags, _)) = find_property_slot_by_name_id(caller, t_ptr, tk_c)
            {
                let configurable = (flags & constants::FLAG_CONFIGURABLE) != 0;
                if !configurable && !trap_keys_str.contains(tk) {
                    return make_type_error_exception(
                        caller,
                        &format!(
                            "Proxy ownKeys invariant violated: non-configurable property '{}' is missing in trap result",
                            tk
                        ),
                    );
                }
            }
        }
        for &sym_key in &target_keys_sym {
            let Some(name_id) = symbol_value_to_name_id(sym_key) else {
                continue;
            };
            if let Some((_, flags, _)) = find_property_slot_by_name_id(caller, t_ptr, name_id) {
                let configurable = (flags & constants::FLAG_CONFIGURABLE) != 0;
                if !configurable
                    && !trap_keys_sym
                        .iter()
                        .any(|&s| same_value_zero(caller, s, sym_key))
                {
                    return make_type_error_exception(
                        caller,
                        "Proxy ownKeys invariant violated: non-configurable Symbol property is missing in trap result",
                    );
                }
            }
        }
    }
    let len = keys.len() as u32;
    let arr = alloc_array(caller, len);
    for (i, &key) in keys.iter().enumerate() {
        set_array_elem(caller, arr, i as i32, key);
    }
    if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
        write_array_length(caller, arr_ptr, len);
    }
    arr
}

fn descriptor_enumerable(caller: &mut Caller<'_, RuntimeState>, descriptor: i64) -> Option<bool> {
    #[cfg(feature = "managed-heap-v2")]
    if caller
        .data()
        .heap_access_v2()
        .resolve_handle(value::decode_handle(descriptor))
        .is_ok()
    {
        return read_host_data_property_v2(caller, descriptor, "enumerable")
            .map(|value| !value::is_falsy(value));
    }
    let descriptor = resolve_handle(caller, descriptor)?;
    read_object_property_by_name(caller, descriptor, "enumerable")
        .map(|value| !value::is_falsy(value))
}

/// 通过 Reflect.getOwnPropertyDescriptor（含 proxy 陷阱）判断 enumerable。
async fn descriptor_enumerable_on_proxy_async(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    key: i64,
) -> bool {
    let desc = reflect_get_own_property_descriptor_on_object_async(caller, obj, key).await;
    if !value::is_undefined(desc)
        && let Some(enumerable) = descriptor_enumerable(caller, desc)
    {
        return enumerable;
    }
    // 陷阱描述符解析失败时，回退到 target 上的 enumerable（与常见 ownKeys+getOwnPropertyDescriptor 转发 handler 一致）
    if value::is_proxy(obj) {
        let handle = value::decode_proxy_handle(obj) as usize;
        let entry = caller
            .data()
            .proxy_table
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(handle)
            .cloned();
        if let Some(entry) = entry {
            let target_desc = reflect_get_own_property_descriptor_impl(caller, entry.target, key);
            if !value::is_undefined(target_desc)
                && let Some(enumerable) = descriptor_enumerable(caller, target_desc)
            {
                return enumerable;
            }
        }
    }
    false
}

async fn reflect_get_own_property_descriptor_on_object_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
) -> i64 {
    if value::is_proxy(target) {
        let handle = value::decode_proxy_handle(target) as usize;
        let entry = {
            let table = caller
                .data()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if let Some(exc) = check_proxy_revoked(caller, &entry, "getOwnPropertyDescriptor") {
                return exc;
            }
            #[cfg(feature = "managed-heap-v2")]
            let trap =
                read_host_data_property_v2(caller, entry.handler, "getOwnPropertyDescriptor")
                    .unwrap_or_else(value::encode_undefined);
            #[cfg(not(feature = "managed-heap-v2"))]
            let trap = resolve_handle(caller, entry.handler)
                .and_then(|handler| {
                    read_object_property_by_name(caller, handler, "getOwnPropertyDescriptor")
                })
                .unwrap_or_else(value::encode_undefined);
            if !value::is_undefined(trap) && !value::is_null(trap) {
                let descriptor = match call_wasm_callback_async(
                    caller,
                    trap,
                    entry.handler,
                    &[entry.target, prop],
                )
                .await
                {
                    Ok(descriptor) => descriptor,
                    Err(error) => {
                        set_runtime_error(
                            caller.data(),
                            format!("TypeError: getOwnPropertyDescriptor trap failed: {error}"),
                        );
                        return value::encode_undefined();
                    }
                };
                let prop_name = render_value(caller, prop).ok();
                let name_id = prop_name
                    .as_deref()
                    .and_then(|name| find_memory_c_string(caller, name));
                if let Err(error) = validate_proxy_get_own_property_descriptor_result(
                    caller,
                    entry.target,
                    name_id,
                    descriptor,
                ) {
                    set_runtime_error(caller.data(), error);
                    return value::encode_undefined();
                }
                return descriptor;
            }
            return reflect_get_own_property_descriptor_impl(caller, entry.target, prop);
        }
        return value::encode_undefined();
    }
    reflect_get_own_property_descriptor_impl(caller, target, prop)
}

/// Object.keys：proxy 走 ownKeys 陷阱后按 enumerable 过滤字符串键。
pub(crate) async fn object_enumerable_own_keys_async(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
) -> i64 {
    if !value::is_js_object(obj) {
        return alloc_array(caller, 0);
    }
    if value::is_proxy(obj) {
        let keys_arr = proxy_own_keys_trap_async(caller, obj).await;
        if value::is_exception(keys_arr) {
            return keys_arr;
        }
        if value::is_undefined(keys_arr) {
            return alloc_array(caller, 0);
        }
        let keys = match extract_array_like_elements(caller, keys_arr).await {
            Ok(k) => k,
            Err(_) => return alloc_array(caller, 0),
        };
        let mut out = Vec::new();
        for key in keys {
            if value::is_symbol(key) {
                continue;
            }
            if descriptor_enumerable_on_proxy_async(caller, obj, key).await {
                out.push(key);
            }
        }
        let len = out.len() as u32;
        let arr = alloc_array(caller, len);
        for (i, key) in out.into_iter().enumerate() {
            set_array_elem(caller, arr, i as i32, key);
        }
        if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
            write_array_length(caller, arr_ptr, len);
        }
        return arr;
    }
    let Some(ptr) = resolve_handle(caller, obj) else {
        return alloc_array(caller, 0);
    };
    let keys = collect_own_property_string_key_values(caller, obj, ptr, true);
    let len = keys.len() as u32;
    let arr = alloc_array(caller, len);
    for (i, key) in keys.into_iter().enumerate() {
        set_array_elem(caller, arr, i as i32, key);
    }
    if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
        write_array_length(caller, arr_ptr, len);
    }
    arr
}

/// Object.getOwnPropertyNames：proxy 走 ownKeys 陷阱，仅保留字符串键。
pub(crate) async fn object_get_own_property_names_async(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
) -> i64 {
    if !value::is_js_object(obj) {
        return alloc_array(caller, 0);
    }
    if value::is_proxy(obj) {
        let keys_arr = proxy_own_keys_trap_async(caller, obj).await;
        if value::is_exception(keys_arr) {
            return keys_arr;
        }
        if value::is_undefined(keys_arr) {
            return alloc_array(caller, 0);
        }
        let keys = match extract_array_like_elements(caller, keys_arr).await {
            Ok(k) => k,
            Err(_) => return alloc_array(caller, 0),
        };
        let mut out = Vec::new();
        for key in keys {
            if !value::is_symbol(key) {
                out.push(key);
            }
        }
        let len = out.len() as u32;
        let arr = alloc_array(caller, len);
        for (i, key) in out.into_iter().enumerate() {
            set_array_elem(caller, arr, i as i32, key);
        }
        if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
            write_array_length(caller, arr_ptr, len);
        }
        return arr;
    }
    let Some(ptr) = resolve_handle(caller, obj) else {
        return alloc_array(caller, 0);
    };
    let keys = collect_own_property_string_key_values(caller, obj, ptr, false);
    let len = keys.len() as u32;
    let arr = alloc_array(caller, len);
    for (i, key) in keys.into_iter().enumerate() {
        set_array_elem(caller, arr, i as i32, key);
    }
    if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
        write_array_length(caller, arr_ptr, len);
    }
    arr
}

/// Object.values：enumerable 字符串键 + Reflect.get 取值。
pub(crate) async fn object_values_async(caller: &mut Caller<'_, RuntimeState>, obj: i64) -> i64 {
    if !value::is_js_object(obj) {
        return alloc_array(caller, 0);
    }
    let keys_arr = object_enumerable_own_keys_async(caller, obj).await;
    if value::is_exception(keys_arr) {
        return keys_arr;
    }
    let keys = match extract_array_like_elements(caller, keys_arr).await {
        Ok(k) => k,
        Err(_) => return alloc_array(caller, 0),
    };
    let arr = alloc_array(caller, keys.len() as u32);
    for (i, key) in keys.iter().enumerate() {
        let val = reflect_get_impl_with_receiver_async(caller, obj, *key, obj).await;
        set_array_elem(caller, arr, i as i32, val);
    }
    if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
        write_array_length(caller, arr_ptr, keys.len() as u32);
    }
    arr
}

/// Object.getOwnPropertySymbols：proxy 走 ownKeys 陷阱，仅保留 Symbol 键。
pub(crate) async fn object_get_own_property_symbols_async(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
) -> i64 {
    if !value::is_js_object(obj) {
        return alloc_array(caller, 0);
    }
    if value::is_proxy(obj) {
        let keys_arr = proxy_own_keys_trap_async(caller, obj).await;
        if value::is_exception(keys_arr) {
            return keys_arr;
        }
        if value::is_undefined(keys_arr) {
            return alloc_array(caller, 0);
        }
        let keys = match extract_array_like_elements(caller, keys_arr).await {
            Ok(k) => k,
            Err(_) => return alloc_array(caller, 0),
        };
        let out: Vec<i64> = keys
            .into_iter()
            .filter(|key| value::is_symbol(*key))
            .collect();
        let len = out.len() as u32;
        let arr = alloc_array(caller, len);
        for (i, key) in out.into_iter().enumerate() {
            set_array_elem(caller, arr, i as i32, key);
        }
        if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
            write_array_length(caller, arr_ptr, len);
        }
        return arr;
    }
    let Some(ptr) = resolve_handle(caller, obj) else {
        return alloc_array(caller, 0);
    };
    let symbols = collect_own_property_key_values(caller, ptr, true);
    let len = symbols.len() as u32;
    let arr = alloc_array(caller, len);
    for (i, symbol) in symbols.into_iter().enumerate() {
        set_array_elem(caller, arr, i as i32, symbol);
    }
    if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
        write_array_length(caller, arr_ptr, len);
    }
    arr
}

/// Object.entries：enumerable 字符串键 + Reflect.get 取值。
pub(crate) async fn object_entries_async(caller: &mut Caller<'_, RuntimeState>, obj: i64) -> i64 {
    if !value::is_js_object(obj) {
        return alloc_array(caller, 0);
    }
    let keys_arr = object_enumerable_own_keys_async(caller, obj).await;
    let keys = match extract_array_like_elements(caller, keys_arr).await {
        Ok(k) => k,
        Err(_) => return alloc_array(caller, 0),
    };
    let arr = alloc_array(caller, keys.len() as u32);
    for (i, key) in keys.iter().enumerate() {
        let val = reflect_get_impl_with_receiver_async(caller, obj, *key, obj).await;
        let pair = alloc_array(caller, 2);
        set_array_elem(caller, pair, 0, *key);
        set_array_elem(caller, pair, 1, val);
        set_array_elem(caller, arr, i as i32, pair);
    }
    if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
        write_array_length(caller, arr_ptr, keys.len() as u32);
    }
    arr
}

fn proxy_type_error(caller: &mut Caller<'_, RuntimeState>, msg: &'static str) -> i64 {
    let msg_val = store_runtime_string(caller, msg.to_string());
    let error_obj = create_error_object(caller, "TypeError", msg_val, value::encode_undefined());
    let mut errors = caller
        .data()
        .error_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let idx = errors.len() as u32;
    errors.push(crate::ErrorEntry {
        name: "TypeError".to_string(),
        message: msg.to_string(),
        value: error_obj,
    });
    value::encode_handle(value::TAG_EXCEPTION, idx)
}

pub(crate) fn define_proxy_reflect(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let proxy_create_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, handler: i64| -> i64 {
            if !value::is_js_object(target) {
                return proxy_type_error(&mut caller, "TypeError: Proxy target must be an object");
            }
            if !value::is_js_object(handler) {
                return proxy_type_error(&mut caller, "TypeError: Proxy handler must be an object");
            }
            let handle;
            {
                let mut table = caller
                    .data()
                    .proxy_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                handle = table.len() as u32;
                table.push(ProxyEntry {
                    target,
                    handler,
                    revoked: false,
                });
            }
            value::encode_proxy_handle(handle)
        },
    );
    linker.define(&mut store, "env", "proxy_create", proxy_create_fn)?;

    let proxy_revocable_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, handler: i64| -> i64 {
            if !value::is_js_object(target) {
                return proxy_type_error(&mut caller, "TypeError: Proxy target must be an object");
            }
            if !value::is_js_object(handler) {
                return proxy_type_error(&mut caller, "TypeError: Proxy handler must be an object");
            }
            let handle = {
                let mut table = caller
                    .data()
                    .proxy_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let handle = table.len() as u32;
                table.push(ProxyEntry {
                    target,
                    handler,
                    revoked: false,
                });
                handle
            };
            let proxy_val = value::encode_proxy_handle(handle);
            let revoke_fn = {
                let mut native_callables = caller
                    .data()
                    .native_callables
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let idx = native_callables.len() as u32;
                native_callables.push(NativeCallable::ProxyRevoker {
                    proxy_handle: handle,
                });
                value::encode_native_callable_idx(idx)
            };
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 2)
            };
            let _ = define_host_data_property_from_caller(&mut caller, obj, "proxy", proxy_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "revoke", revoke_fn);
            obj
        },
    );
    linker.define(&mut store, "env", "proxy_revocable", proxy_revocable_fn)?;

    Ok(())
}
