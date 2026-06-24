use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Extern, Func, Linker, Val};

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

pub(crate) fn reflect_set_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
    val: i64,
) -> i64 {
    let Ok(prop_name) = render_value(caller, prop) else {
        return value::encode_bool(false);
    };
    let name_id = find_memory_c_string(caller, &prop_name);
    let existing = name_id.and_then(|id| {
        let obj_ptr = resolve_handle(caller, target)?;
        find_property_slot_by_name_id(caller, obj_ptr, id)
    });
    if let Some((_, flags, _)) = existing {
        let is_accessor = (flags & constants::FLAG_IS_ACCESSOR) != 0;
        if !is_accessor {
            let writable = (flags & constants::FLAG_WRITABLE) != 0;
            if !writable {
                return value::encode_bool(false);
            }
        }
    } else if !is_extensible_impl(caller, target) {
        return value::encode_bool(false);
    }
    let _ = define_host_data_property_from_caller(caller, target, &prop_name, val);
    value::encode_bool(true)
}

pub(crate) fn reflect_has_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
) -> i64 {
    let obj_ptr = resolve_handle(caller, target);
    if let Some(ptr) = obj_ptr
        && let Ok(prop_name) = render_value(caller, prop)
        && let Some(name_id) = find_memory_c_string(caller, &prop_name)
    {
        let found = find_property_slot_by_name_id(caller, ptr, name_id).is_some();
        return value::encode_bool(found);
    }
    value::encode_bool(false)
}

pub(crate) fn reflect_delete_property_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
) -> i64 {
    let prop_name = match render_value(caller, prop) {
        Ok(name) => name,
        Err(_) => return value::encode_bool(true),
    };
    let Some(ptr) = resolve_handle(caller, target) else {
        return value::encode_bool(true);
    };
    let Some(name_id) = find_memory_c_string(caller, &prop_name) else {
        return value::encode_bool(true);
    };
    let Some((slot_offset, flags, _val)) = find_property_slot_by_name_id(caller, ptr, name_id)
    else {
        return value::encode_bool(true);
    };
    // Not configurable → can't delete
    if (flags & constants::FLAG_CONFIGURABLE) == 0 {
        return value::encode_bool(false);
    }
    // Perform swap-remove
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return value::encode_bool(false);
    };
    let data = memory.data_mut(&mut *caller);
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
    let last_slot_offset = ptr + 16 + (num_props - 1) * 32;
    // Decrement num_props
    data[ptr + 12..ptr + 16].copy_from_slice(&(num_props as u32 - 1).to_le_bytes());
    // If not deleting the last slot, copy last slot over deleted slot
    if slot_offset != last_slot_offset {
        for j in 0..32 {
            data[slot_offset + j] = data[last_slot_offset + j];
        }
    }
    value::encode_bool(true)
}

pub(crate) async fn extract_array_like_elements(
    caller: &mut Caller<'_, RuntimeState>,
    arr_like: i64,
) -> Result<Vec<i64>, String> {
    let mut elements = Vec::new();
    if value::is_array(arr_like) {
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
    let shadow_sp_global = caller
        .get_export("__shadow_sp")
        .and_then(|e| e.into_global())
        .unwrap();
    let saved_sp = shadow_sp_global.get(&mut *caller).i32().unwrap();
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .unwrap();
    for (i, &arg) in args.iter().enumerate() {
        memory
            .write(
                &mut *caller,
                (saved_sp + i as i32 * 8) as usize,
                &arg.to_le_bytes(),
            )
            .unwrap();
    }
    shadow_sp_global
        .set(&mut *caller, Val::I32(saved_sp + args.len() as i32 * 8))
        .unwrap();
    let result =
        resolve_and_call_async(caller, target, this_arg, saved_sp, args.len() as i32).await;
    shadow_sp_global
        .set(&mut *caller, Val::I32(saved_sp))
        .unwrap();
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

    let shadow_sp_global = caller
        .get_export("__shadow_sp")
        .and_then(|e| e.into_global())
        .expect("__shadow_sp in reflect_construct_impl_async");
    let saved_sp = shadow_sp_global
        .get(&mut *caller)
        .i32()
        .expect("shadow_sp i32 in reflect_construct_impl_async");
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .expect("memory in reflect_construct_impl_async");
    for (i, &arg) in args.iter().enumerate() {
        memory
            .write(
                &mut *caller,
                (saved_sp + i as i32 * 8) as usize,
                &arg.to_le_bytes(),
            )
            .expect("shadow stack write in reflect_construct_impl_async");
    }
    shadow_sp_global
        .set(&mut *caller, Val::I32(saved_sp + args.len() as i32 * 8))
        .expect("shadow_sp set in reflect_construct_impl_async");
    let result =
        resolve_and_call_async(caller, target, this_obj, saved_sp, args.len() as i32).await;
    shadow_sp_global
        .set(&mut *caller, Val::I32(saved_sp))
        .expect("shadow_sp restore in reflect_construct_impl_async");

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
            let table = caller.data().proxy_table.lock().unwrap_or_else(|e| e.into_inner());
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
    let Some(ptr) = resolve_handle(caller, target) else {
        return value::encode_bool(false);
    };
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return value::encode_bool(false);
    };

    let proto_handle = if value::is_null(proto) {
        0xFFFF_FFFF
    } else if value::is_object(proto) {
        value::decode_object_handle(proto)
    } else if value::is_array(proto) {
        value::decode_array_handle(proto)
    } else if value::is_proxy(proto) {
        value::decode_proxy_handle(proto)
    } else if value::is_function(proto) || value::is_closure(proto) {
        value::decode_object_handle(proto)
    } else {
        0xFFFF_FFFF
    };

    let data = memory.data_mut(&mut *caller);
    if ptr + 4 > data.len() {
        return value::encode_bool(false);
    }
    data[ptr..ptr + 4].copy_from_slice(&proto_handle.to_le_bytes());
    value::encode_bool(true)
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
        let table = caller.data().proxy_table.lock().unwrap_or_else(|e| e.into_inner());
        table.get(handle).cloned()
    };
    let Some(entry) = entry else {
        return value::encode_undefined();
    };
    if let Some(exc) = check_proxy_revoked(caller, &entry, "ownKeys") {
        return exc;
    }
    let Some(handler_ptr) = resolve_handle(caller, entry.handler) else {
        return reflect_own_keys_impl(caller, entry.target);
    };
    let trap = read_object_property_by_name(caller, handler_ptr, "ownKeys")
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
    let Some(t_ptr) = resolve_handle(caller, entry.target) else {
        return value::encode_undefined();
    };
    let target_keys_str = collect_own_property_names(caller, t_ptr, false);
    let target_keys_sym: Vec<i64> =
        collect_own_property_key_values(caller, t_ptr, true);
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
                if !trap_keys_sym.iter().any(|&s| same_value_zero(s, tk)) {
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
                && let Some((_, flags, _)) =
                    find_property_slot_by_name_id(caller, t_ptr, tk_c)
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
                    && !trap_keys_sym.iter().any(|&s| same_value_zero(s, sym_key))
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

/// 通过 Reflect.getOwnPropertyDescriptor（含 proxy 陷阱）判断 enumerable。
async fn descriptor_enumerable_on_proxy_async(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    key: i64,
) -> bool {
    let desc = reflect_get_own_property_descriptor_on_object_async(caller, obj, key).await;
    if !value::is_undefined(desc)
        && let Some(desc_ptr) = resolve_handle(caller, desc)
    {
        let prop_enum = read_object_property_by_name(caller, desc_ptr, "enumerable");
        return prop_enum.is_some_and(|v| !value::is_falsy(v));
    }
    // 陷阱描述符解析失败时，回退到 target 上的 enumerable（与常见 ownKeys+getOwnPropertyDescriptor 转发 handler 一致）
    if value::is_proxy(obj) {
        let handle = value::decode_proxy_handle(obj) as usize;
        let entry = caller
            .data()
            .proxy_table.lock().unwrap_or_else(|e| e.into_inner())
            .get(handle)
            .cloned();
        if let Some(entry) = entry {
            let target_desc = reflect_get_own_property_descriptor_impl(caller, entry.target, key);
            if !value::is_undefined(target_desc)
                && let Some(desc_ptr) = resolve_handle(caller, target_desc)
            {
                let prop_enum = read_object_property_by_name(caller, desc_ptr, "enumerable");
                return prop_enum.is_some_and(|v| !value::is_falsy(v));
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
            let table = caller.data().proxy_table.lock().unwrap_or_else(|e| e.into_inner());
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if let Some(exc) = check_proxy_revoked(caller, &entry, "getOwnPropertyDescriptor") {
                return exc;
            }
            if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                let trap =
                    read_object_property_by_name(caller, handler_ptr, "getOwnPropertyDescriptor")
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
                        Ok(desc) => desc,
                        Err(e) => {
                            set_runtime_error(
                                caller.data(),
                                format!("TypeError: getOwnPropertyDescriptor trap failed: {}", e),
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
    let names = collect_own_property_names(caller, ptr, true);
    let len = names.len() as u32;
    let arr = alloc_array(caller, len);
    for (i, name) in names.into_iter().enumerate() {
        let name_val = store_runtime_string(caller, name);
        set_array_elem(caller, arr, i as i32, name_val);
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
    let names = collect_own_property_names(caller, ptr, false);
    let len = names.len() as u32;
    let arr = alloc_array(caller, len);
    for (i, name) in names.into_iter().enumerate() {
        let name_val = store_runtime_string(caller, name);
        set_array_elem(caller, arr, i as i32, name_val);
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
    let error_obj = create_error_object(caller, "TypeError", msg_val);
    let mut errors = caller.data().error_table.lock().unwrap_or_else(|e| e.into_inner());
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
                let mut table = caller.data().proxy_table.lock().unwrap_or_else(|e| e.into_inner());
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
                let mut table = caller.data().proxy_table.lock().unwrap_or_else(|e| e.into_inner());
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
                let mut native_callables = caller.data().native_callables.lock().unwrap_or_else(|e| e.into_inner());
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
