use super::*;
/// ECMAScript §7.1.4 ToNumber for sync host imports (e.g. Math).
pub(crate) fn value_to_number_wasm(
    caller: &mut Caller<'_, RuntimeState>,
    arg: i64,
) -> Result<f64, i64> {
    let number = to_number(caller, arg);
    if value::is_exception(number) {
        Err(number)
    } else {
        Ok(value::decode_f64(number))
    }
}

pub(crate) fn value_to_number_or_exception(caller: &mut Caller<'_, RuntimeState>, arg: i64) -> i64 {
    match value_to_number_wasm(caller, arg) {
        Ok(x) => value::encode_f64(x),
        Err(exc) => exc,
    }
}

pub(crate) fn is_callable_in_runtime(caller: &mut Caller<'_, RuntimeState>, val: i64) -> bool {
    if value::is_callable(val) {
        return true;
    }
    if value::is_proxy(val) {
        let handle = value::decode_proxy_handle(val) as usize;
        let entry = {
            let table = caller
                .data()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
            let table = caller
                .data()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
            let table = caller
                .data()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
    let key = target as u64;
    if caller
        .data()
        .non_extensible_handles
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&key)
    {
        return false;
    }
    if caller
        .data()
        .heap_access_v2()
        .resolve_handle(value::decode_handle(target))
        .is_ok()
    {
        // V2 对象的 non-extensible 状态由 `non_extensible_handles` side table 承载，
        // 上方未命中即为 extensible（ManagedHeap 无独立 non-extensible header flag）。
        return true;
    }
    if let Some(ptr) = resolve_handle(caller, target) {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return true;
        };
        let data = memory.data(&*caller);
        if ptr + 6 <= data.len() && data[ptr + 5] != 0 {
            return false;
        }
    }
    true
}

pub(crate) fn prevent_extensions_impl(caller: &mut Caller<'_, RuntimeState>, target: i64) -> bool {
    if !value::is_js_object(target) {
        return false;
    }
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
            if entry.revoked {
                return false;
            }
            return prevent_extensions_impl(caller, entry.target);
        }
        return false;
    }
    {
        let mut set = caller
            .data()
            .non_extensible_handles
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set.insert(target as u64);
    }
    {
        // V2 non-extensible 状态只由 side table 承载；resolve_handle 在 V2 下返回
        // handle id 而非线性内存指针，继续写 `ptr + 5` 会破坏 data segment。
    }
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

fn descriptor_properties(
    caller: &mut Caller<'_, RuntimeState>,
    descriptor: i64,
) -> Option<[Option<i64>; 6]> {
    if caller
        .data()
        .heap_access_v2()
        .resolve_handle(value::decode_handle(descriptor))
        .is_ok()
    {
        return Some([
            read_host_data_property_v2(caller, descriptor, "value"),
            read_host_data_property_v2(caller, descriptor, "writable"),
            read_host_data_property_v2(caller, descriptor, "enumerable"),
            read_host_data_property_v2(caller, descriptor, "configurable"),
            read_host_data_property_v2(caller, descriptor, "get"),
            read_host_data_property_v2(caller, descriptor, "set"),
        ]);
    }
    let descriptor_ptr = resolve_handle(caller, descriptor)?;
    Some([
        read_object_property_by_name(caller, descriptor_ptr, "value"),
        read_object_property_by_name(caller, descriptor_ptr, "writable"),
        read_object_property_by_name(caller, descriptor_ptr, "enumerable"),
        read_object_property_by_name(caller, descriptor_ptr, "configurable"),
        read_object_property_by_name(caller, descriptor_ptr, "get"),
        read_object_property_by_name(caller, descriptor_ptr, "set"),
    ])
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
        return Err("Invalid property descriptor".to_string());
    }
    let Some(
        [
            prop_value,
            prop_writable,
            prop_enumerable,
            prop_configurable,
            prop_get,
            prop_set,
        ],
    ) = descriptor_properties(caller, desc_handle)
    else {
        return Err("Invalid property descriptor".to_string());
    };

    if let Some(getter) = prop_get
        && !value::is_undefined(getter)
        && !value::is_null(getter)
        && !is_callable_in_runtime(caller, getter)
    {
        return Err("property getter must be callable".to_string());
    }
    if let Some(setter) = prop_set
        && !value::is_undefined(setter)
        && !value::is_null(setter)
        && !is_callable_in_runtime(caller, setter)
    {
        return Err("property setter must be callable".to_string());
    }

    let has_accessor = prop_get.is_some() || prop_set.is_some();
    if has_accessor {
        if prop_value.is_some() {
            return Err(
                "Invalid property descriptor: cannot specify both accessor and value".to_string(),
            );
        }
        if prop_writable.is_some() {
            return Err(
                "Invalid property descriptor: cannot specify both accessor and writable"
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

pub(crate) fn is_accessor_descriptor(desc: &PropertyDescriptor) -> bool {
    desc.get.is_some() || desc.set.is_some()
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
