use super::*;
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
    if value::is_callable(val) {
        return true;
    }
    if value::is_proxy(val) {
        let handle = value::decode_proxy_handle(val) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        if let Some(entry) = entry
            && !entry.revoked
        {
            return is_callable_in_runtime(caller, entry.target);
        }
    }
    false
}

pub(crate) fn is_constructor_in_runtime(caller: &mut Caller<'_, RuntimeState>, val: i64) -> bool {
    if value::is_callable(val) {
        return true;
    }
    if value::is_proxy(val) {
        let handle = value::decode_proxy_handle(val) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        };
        if let Some(entry) = entry
            && !entry.revoked
        {
            return is_constructor_in_runtime(caller, entry.target);
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
    let function_props_base = caller
        .get_export("__function_props_base")
        .and_then(Extern::into_global)
        .and_then(|global| global.get(&mut *caller).i32())
        .unwrap_or(0) as u32;
    if proto_handle >= function_props_base && proto_handle < function_props_base + num_ir_functions
    {
        value::encode_function_idx(proto_handle - function_props_base)
    } else {
        value::encode_object_handle(proto_handle)
    }
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
pub(crate) fn parse_descriptor(
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

    if let Some(getter) = prop_get
        && !value::is_undefined(getter)
        && !value::is_null(getter)
        && !is_callable_in_runtime(caller, getter)
    {
        return Err("TypeError: property getter must be callable".to_string());
    }
    if let Some(setter) = prop_set
        && !value::is_undefined(setter)
        && !value::is_null(setter)
        && !is_callable_in_runtime(caller, setter)
    {
        return Err("TypeError: property setter must be callable".to_string());
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
pub(crate) fn get_target_descriptor(
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

pub(crate) fn is_accessor_descriptor(desc: &PropertyDescriptor) -> bool {
    desc.get.is_some() || desc.set.is_some()
}

pub(crate) fn is_data_descriptor(desc: &PropertyDescriptor) -> bool {
    desc.value.is_some() || desc.writable.is_some()
}

pub(crate) fn complete_property_descriptor(mut desc: PropertyDescriptor) -> PropertyDescriptor {
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

pub(crate) fn descriptor_value_same(caller: &mut Caller<'_, RuntimeState>, left: i64, right: i64) -> bool {
    !value::is_falsy(strict_eq(caller, left, right))
}

pub(crate) fn is_compatible_property_descriptor(
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
