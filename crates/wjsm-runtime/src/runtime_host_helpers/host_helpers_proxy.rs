use super::*;

fn proxy_handler_trap(caller: &mut Caller<'_, RuntimeState>, handler: i64, name: &str) -> i64 {
    #[cfg(feature = "managed-heap-v2")]
    {
        return read_host_data_property_v2(caller, handler, name)
            .unwrap_or_else(value::encode_undefined);
    }
    #[cfg(not(feature = "managed-heap-v2"))]
    resolve_handle(caller, handler)
        .and_then(|handler_ptr| read_object_property_by_name(caller, handler_ptr, name))
        .unwrap_or_else(value::encode_undefined)
}

pub(crate) fn define_property_on_normal_object(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: u32,
    desc: &PropertyDescriptor,
) -> Result<bool, String> {
    if value::is_array(target) {
        if desc.get.is_some() || desc.set.is_some() {
            return Err(
                "TypeError: Accessor properties are not supported on array symbol slots"
                    .to_string(),
            );
        }
        return crate::array_named_props::define_data_property_on_array_named(
            caller, target, name_id, desc,
        );
    }

    // V2 对象堆在 memory64；resolve_handle 返回 handle id 而非 memory32 指针，
    // 必须走 heap_access_v2，禁止继续写 main memory 槽。
    // 函数/闭包/bound 用 function_props handle（handle_index_of）。
    #[cfg(feature = "managed-heap-v2")]
    {
        let handle = handle_index_of(caller, target) as u32;
        if caller
            .data()
            .heap_access_v2()
            .resolve_handle(handle)
            .is_ok()
        {
            return define_property_on_v2_object(caller, target, name_id, desc);
        }
    }

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
            if let Some(new_enum) = desc.enumerable
                && new_enum != old_enumerable
            {
                return Err(
                    "TypeError: Cannot redefine enumerable attribute of non-configurable property"
                        .to_string(),
                );
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
                if let Some(new_getter) = desc.get
                    && new_getter != old_getter
                {
                    return Err(
                        "TypeError: Cannot change getter of non-configurable property".to_string(),
                    );
                }
                if let Some(new_setter) = desc.set
                    && new_setter != old_setter
                {
                    return Err(
                        "TypeError: Cannot change setter of non-configurable property".to_string(),
                    );
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

        let Some(env) = WasmEnv::from_caller(caller) else {
            return Ok(false);
        };
        {
            let data = env.memory.data_mut(&mut *caller);
            data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
        }
        let handle = handle_index_of(caller, target) as u32;
        let slot_idx = (slot_offset - (obj_ptr + 16)) / 32;
        let _ = crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle,
            slot_idx,
            crate::runtime_gc::heap_access::SlotPart::Value,
            val,
        );
        let _ = crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle,
            slot_idx,
            crate::runtime_gc::heap_access::SlotPart::Getter,
            getter,
        );
        let _ = crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle,
            slot_idx,
            crate::runtime_gc::heap_access::SlotPart::Setter,
            setter,
        );

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

/// V2 DefineProperty：读写均走 heap_access_v2，含 non-configurable invariants。
#[cfg(feature = "managed-heap-v2")]
fn define_property_on_v2_object(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: u32,
    desc: &PropertyDescriptor,
) -> Result<bool, String> {
    // 函数/闭包/bound 的属性对象 handle 从 function_props_base 起算。
    let handle = handle_index_of(caller, target) as u32;
    let access = caller.data().heap_access_v2().clone();
    let key = crate::property_key::canonicalize_v2_name_id(caller, name_id)
        .ok_or_else(|| "TypeError: Invalid property key".to_string())?;
    let existing = access
        .get_property_slot(handle, key)
        .map_err(|error| format!("TypeError: {error}"))?;

    if let Some(old) = existing {
        let old_flags = old.flags as i32;
        let old_configurable = (old_flags & constants::FLAG_CONFIGURABLE) != 0;
        let old_enumerable = (old_flags & constants::FLAG_ENUMERABLE) != 0;
        let old_writable = (old_flags & constants::FLAG_WRITABLE) != 0;
        let old_accessor = (old_flags & constants::FLAG_IS_ACCESSOR) != 0;
        let old_val = old.value as i64;
        let old_getter = old.getter as i64;
        let old_setter = old.setter as i64;

        if !old_configurable {
            if desc.configurable == Some(true) {
                return Err("TypeError: Cannot redefine non-configurable property".to_string());
            }
            if let Some(new_enum) = desc.enumerable
                && new_enum != old_enumerable
            {
                return Err(
                    "TypeError: Cannot redefine enumerable attribute of non-configurable property"
                        .to_string(),
                );
            }
            let is_new_accessor = desc.get.is_some() || desc.set.is_some();
            if is_new_accessor != old_accessor {
                return Err("TypeError: Cannot change property type from data to accessor or vice versa on non-configurable property".to_string());
            }
            if !old_accessor {
                if !old_writable {
                    if desc.writable == Some(true) {
                        return Err(
                            "TypeError: Cannot make non-writable property writable".to_string(),
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
                if let Some(new_getter) = desc.get
                    && new_getter != old_getter
                {
                    return Err(
                        "TypeError: Cannot change getter of non-configurable property".to_string(),
                    );
                }
                if let Some(new_setter) = desc.set
                    && new_setter != old_setter
                {
                    return Err(
                        "TypeError: Cannot change setter of non-configurable property".to_string(),
                    );
                }
            }
        }

        let is_accessor = desc.get.is_some()
            || desc.set.is_some()
            || (desc.value.is_none() && desc.writable.is_none() && old_accessor);
        let mut flags: u32 = 0;
        if is_accessor {
            flags |= constants::FLAG_IS_ACCESSOR as u32;
        }
        let writable = desc
            .writable
            .unwrap_or(if !is_accessor { old_writable } else { false });
        if writable {
            flags |= constants::FLAG_WRITABLE as u32;
        }
        if desc.enumerable.unwrap_or(old_enumerable) {
            flags |= constants::FLAG_ENUMERABLE as u32;
        }
        if desc.configurable.unwrap_or(old_configurable) {
            flags |= constants::FLAG_CONFIGURABLE as u32;
        }

        if is_accessor {
            let getter = desc.get.unwrap_or(old_getter);
            let setter = desc.set.unwrap_or(old_setter);
            access
                .define_accessor_property_with_flags(
                    handle,
                    key,
                    getter as u64,
                    setter as u64,
                    flags,
                )
                .map_err(|error| format!("TypeError: {error}"))?;
        } else {
            let val = desc.value.unwrap_or(old_val);
            access
                .define_data_property(handle, key, val as u64, flags)
                .map_err(|error| format!("TypeError: {error}"))?;
        }
        Ok(true)
    } else {
        if !is_extensible_impl(caller, target) {
            return Err("TypeError: Cannot add property to non-extensible object".to_string());
        }
        let is_accessor = desc.get.is_some() || desc.set.is_some();
        let mut flags: u32 = 0;
        if is_accessor {
            flags |= constants::FLAG_IS_ACCESSOR as u32;
        }
        if desc.writable.unwrap_or(false) && !is_accessor {
            flags |= constants::FLAG_WRITABLE as u32;
        }
        if desc.enumerable.unwrap_or(false) {
            flags |= constants::FLAG_ENUMERABLE as u32;
        }
        if desc.configurable.unwrap_or(false) {
            flags |= constants::FLAG_CONFIGURABLE as u32;
        }
        if is_accessor {
            let getter = desc.get.unwrap_or(value::encode_undefined());
            let setter = desc.set.unwrap_or(value::encode_undefined());
            access
                .define_accessor_property_with_flags(
                    handle,
                    key,
                    getter as u64,
                    setter as u64,
                    flags,
                )
                .map_err(|error| format!("TypeError: {error}"))?;
        } else {
            let val = desc.value.unwrap_or(value::encode_undefined());
            access
                .define_data_property(handle, key, val as u64, flags)
                .map_err(|error| format!("TypeError: {error}"))?;
        }
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
    let Some(env) = WasmEnv::from_caller(caller) else {
        return;
    };
    let handle_idx = crate::runtime_values::handle_index_of(caller, target) as u32;

    if num_props >= capacity {
        let Some(new_capacity) = capacity.max(1).checked_mul(2) else {
            return;
        };
        let Some(new_size) = new_capacity
            .checked_mul(32)
            .and_then(|payload| 16_usize.checked_add(payload))
        else {
            return;
        };

        let Some(heap_ptr) = crate::runtime_heap::alloc_heap_region_for_host(
            caller,
            &env,
            new_size,
            wjsm_ir::HEAP_TYPE_OBJECT,
            new_capacity as u32,
        ) else {
            return;
        };
        let Some(current_obj_ptr) =
            crate::runtime_values::resolve_handle_idx_with_env(caller, &env, handle_idx as usize)
        else {
            return;
        };
        actual_obj_ptr = current_obj_ptr;
        let obj_table_ptr = env.obj_table_ptr.get(&mut *caller).i32().unwrap_or(0) as usize;
        {
            let data = env.memory.data_mut(&mut *caller);

            let old_size = 16 + num_props * 32;
            data.copy_within(actual_obj_ptr..actual_obj_ptr + old_size, heap_ptr);

            data[heap_ptr + 8..heap_ptr + 12].copy_from_slice(&(new_capacity as u32).to_le_bytes());

            let slot_addr = obj_table_ptr + handle_idx as usize * 4;
            if slot_addr + 4 <= data.len() {
                data[slot_addr..slot_addr + 4].copy_from_slice(&(heap_ptr as u32).to_le_bytes());
            }
        }

        actual_obj_ptr = heap_ptr;
    }

    let slot_idx = num_props;
    let slot_offset = actual_obj_ptr + 16 + slot_idx * 32;
    {
        let data = env.memory.data_mut(&mut *caller);
        if slot_offset + 32 > data.len() {
            return;
        }
        data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
        data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    }
    let _ = crate::runtime_gc::heap_access::write_property_slot(
        caller,
        &env,
        handle_idx,
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Value,
        val,
    );
    let _ = crate::runtime_gc::heap_access::write_property_slot(
        caller,
        &env,
        handle_idx,
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Getter,
        getter,
    );
    let _ = crate::runtime_gc::heap_access::write_property_slot(
        caller,
        &env,
        handle_idx,
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Setter,
        setter,
    );
    let data = env.memory.data_mut(&mut *caller);
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
            let table = caller
                .data()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                return make_type_error_exception(
                    caller,
                    "TypeError: Cannot perform 'getPrototypeOf' on a proxy that has been revoked",
                );
            }
            let trap = proxy_handler_trap(caller, entry.handler, "getPrototypeOf");
            if !value::is_undefined(trap) && !value::is_null(trap) {
                let result =
                    match call_wasm_callback_async(caller, trap, entry.handler, &[entry.target])
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
                        "TypeError: Proxy getPrototypeOf must return an object or null".to_string(),
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
            return Box::pin(proxy_or_target_get_prototype_of_impl_async(
                caller,
                entry.target,
            ))
            .await;
        }
    }

    // TAG_REGEXP 无 obj_table 条目，其 [[Prototype]] 是 RegExp.prototype 对象，
    // 不能走 resolve_handle（会得到 null）；与 ordinary_has_instance_async 同构地
    // 确保原型就绪后直接返回。
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

    #[cfg(feature = "managed-heap-v2")]
    {
        let handle = handle_index_of(caller, target) as u32;
        let access = caller.data().heap_access_v2();
        if access.resolve_handle(handle).is_ok() {
            return match access.prototype(handle) {
                Ok(proto_handle) => prototype_handle_to_value(caller, proto_handle),
                Err(_) => value::encode_null(),
            };
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
            let table = caller
                .data()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
            let trap = proxy_handler_trap(caller, entry.handler, "isExtensible");
            if !value::is_undefined(trap) && !value::is_null(trap) {
                let trap_res =
                    match call_wasm_callback_async(caller, trap, entry.handler, &[entry.target])
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
            let table = caller
                .data()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
            let trap = proxy_handler_trap(caller, entry.handler, "preventExtensions");
            if !value::is_undefined(trap) && !value::is_null(trap) {
                let trap_res =
                    match call_wasm_callback_async(caller, trap, entry.handler, &[entry.target])
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
        && !value::is_closure(target)
        && !value::is_array(target)
        && !value::is_proxy(target)
    {
        return Err("TypeError: Object.defineProperty called on non-object".to_string());
    }

    let desc = parse_descriptor(caller, descriptor)?;

    let name_id = property_key_value_to_name_id(caller, prop, true)
        .ok_or_else(|| "TypeError: Failed to allocate property name string".to_string())?;

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
            let table = caller
                .data()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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

        let trap = proxy_handler_trap(caller, entry.handler, "defineProperty");
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

            if let Some(td) = target_desc {
                let target_configurable = td.configurable.unwrap_or(true);

                if !target_configurable {
                    if desc.configurable == Some(true) {
                        return Err("TypeError: proxy defineProperty invariant violation: cannot redefine non-configurable property as configurable".to_string());
                    }
                    if let Some(new_enum) = desc.enumerable
                        && Some(new_enum) != td.enumerable
                    {
                        return Err("TypeError: proxy defineProperty invariant violation: cannot change enumerableness of non-configurable property".to_string());
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
            } else {
                if !extensible {
                    return Err("TypeError: proxy defineProperty invariant violation: target is non-extensible and property does not exist".to_string());
                }
                if setting_config_false {
                    return Err("TypeError: proxy defineProperty invariant violation: cannot define non-configurable property if it does not exist on target".to_string());
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

pub(crate) async fn reflect_get_impl_with_receiver_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
    receiver: i64,
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
            if entry.revoked {
                return make_type_error_exception(
                    caller,
                    "TypeError: Cannot perform 'get' on a proxy that has been revoked",
                );
            }
            let trap = proxy_handler_trap(caller, entry.handler, "get");
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

    let prop_name = if value::is_string(prop) {
        get_string_utf8_lossy(caller, prop)
    } else {
        match render_value(caller, prop) {
            Ok(name) => name,
            Err(_) => return value::encode_undefined(),
        }
    };

    if value::is_native_callable(target) {
        if let Some(val) =
            crate::symbol_well_known::native_callable_symbol_constructor_static_property(
                caller, target, &prop_name,
            )
        {
            return val;
        }
        if prop_name != "prototype" {
            let idx = value::decode_native_callable_idx(target) as usize;
            let record = {
                let table = caller
                    .data()
                    .native_callables
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(idx).cloned()
            };
            if let Some(NativeCallable::EvalFunction(func)) = record {
                if prop_name == "length" {
                    return value::encode_f64(func.params.len() as f64);
                }
                if prop_name == "name" {
                    return store_runtime_string(caller, String::new());
                }
            }
            return value::encode_undefined();
        }
        let idx = value::decode_native_callable_idx(target) as usize;
        let record = {
            let table = caller
                .data()
                .native_callables
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table.get(idx).cloned()
        };
        return record
            .as_ref()
            .and_then(|record| crate::runtime_heap::native_callable_prototype(caller, record))
            .unwrap_or_else(value::encode_undefined);
    }

    let name_id = property_key_value_to_name_id(caller, prop, false);
    #[cfg(feature = "managed-heap-v2")]
    if value::is_object(target)
        || value::is_array(target)
        || value::is_function(target)
        || value::is_closure(target)
        || value::is_bound(target)
    {
        let Some(raw_name_id) = name_id else {
            return value::encode_undefined();
        };
        let Some(name_id) = crate::property_key::canonicalize_v2_name_id(caller, raw_name_id)
        else {
            return value::encode_undefined();
        };
        let handle = handle_index_of(caller, target) as u32;
        let access = caller.data().heap_access_v2().clone();
        if access.resolve_handle(handle).is_ok() {
            match access
                .get_property_slot_on_proto_chain(handle, name_id)
                .ok()
                .flatten()
            {
                Some(property)
                    if property.flags & constants::FLAG_IS_ACCESSOR as u32 != 0 =>
                {
                    let getter = property.getter as i64;
                    if value::is_undefined(getter) || value::is_null(getter) {
                        return value::encode_undefined();
                    }
                    return call_wasm_callback_async(caller, getter, receiver, &[])
                        .await
                        .unwrap_or_else(|_| value::encode_undefined());
                }
                Some(property) => return property.value as i64,
                None => return value::encode_undefined(),
            }
        }
    }

    let obj_ptr = match resolve_handle(caller, target) {
        Some(ptr) => ptr,
        None => return value::encode_undefined(),
    };

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
                name_c,
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

// ── Caller → _with_env 薄封装宏 ───────────────────────────────────────
/// 为 `_with_env` 泛型版本生成 `Caller` 薄封装，消除重复的
/// `let env = WasmEnv::from_caller(caller).expect("WasmEnv"); $func_with_env(caller, &env, ...)` 样板。
macro_rules! caller_env_wrapper {
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty =
            $with_env:ident
    ) => {
        $(#[$meta])*
        $vis fn $name(caller: &mut Caller<'_, RuntimeState>, $($arg: $ty),*) -> $ret {
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            $with_env(caller, &env, $($arg),*)
        }
    };
}

#[inline]
pub(crate) fn alloc_array(caller: &mut Caller<'_, RuntimeState>, capacity: u32) -> i64 {
    #[cfg(feature = "managed-heap-v2")]
    {
        return match crate::host_imports::allocate_v2_array_handle(caller, capacity) {
            Ok(handle) => value::encode_handle(value::TAG_ARRAY, handle),
            Err(error) => {
                set_runtime_error(caller.data(), format!("V2 host array allocation: {error}"));
                value::encode_undefined()
            }
        };
    }
    #[cfg(not(feature = "managed-heap-v2"))]
    {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_array_with_env(caller, &env, capacity)
    }
}

#[inline]
pub(crate) fn alloc_object(caller: &mut Caller<'_, RuntimeState>, capacity: u32) -> i64 {
    #[cfg(feature = "managed-heap-v2")]
    {
        return crate::alloc_host_object_v2(caller, capacity);
    }
    #[cfg(not(feature = "managed-heap-v2"))]
    {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_object_with_env(caller, &env, capacity)
    }
}

#[inline]
pub(crate) fn set_array_elem(
    caller: &mut Caller<'_, RuntimeState>,
    arr_val: i64,
    index: i32,
    val: i64,
) {
    #[cfg(feature = "managed-heap-v2")]
    {
        let Ok(index) = u32::try_from(index) else {
            return;
        };
        if let Err(error) =
            crate::set_v2_array_element(caller, value::decode_handle(arr_val), index, val as u64)
        {
            set_runtime_error(caller.data(), format!("V2 host array element: {error}"));
        }
    }
    #[cfg(not(feature = "managed-heap-v2"))]
    {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        set_array_elem_with_env(caller, &env, arr_val, index, val);
    }
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn find_memory_c_string(name: &str) -> Option<u32> = find_memory_c_string_with_env
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn alloc_heap_c_string(name: &str) -> Option<u32> = alloc_heap_c_string_with_env
}

#[inline]
pub(crate) fn alloc_promise(caller: &mut Caller<'_, RuntimeState>, mut entry: PromiseEntry) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    if entry.capture_scope.is_none() {
        entry.capture_scope = {
            let mut hooks = caller
                .data()
                .async_hooks
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            hooks.capture_for_scheduled_callback(0, false)
        };
    }
    let promise = alloc_object_with_env(caller, &env, 0);
    if value::is_object(promise) {
        if !value::is_object(caller.data().promise_prototype) {
            crate::runtime_heap::ensure_promise_prototype_initialized(caller, &env);
        }
        let proto = caller.data().promise_prototype;
        if value::is_object(proto) {
            crate::runtime_heap::set_object_proto_header(caller, &env, promise, proto);
        }
        let handle = value::decode_object_handle(promise) as usize;
        let mut table = caller
            .data()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        insert_promise_entry(&mut table, handle, entry);
    }
    promise
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

/// 定义不可枚举数据属性（configurable + writable，不含 enumerable）。
pub(crate) fn define_host_data_property_non_enumerable(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    val: i64,
) -> Option<()> {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let name_id = find_memory_c_string_with_env(caller, &env, name)
        .or_else(|| alloc_heap_c_string_with_env(caller, &env, name))?;
    define_host_data_property_by_name_id_with_flags(
        caller,
        obj,
        encode_string_name_id(name_id),
        val,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
    )
}

pub(crate) fn define_host_data_property_by_name_id_with_flags(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name_id: u32,
    val: i64,
    flags: i32,
) -> Option<()> {
    #[cfg(feature = "managed-heap-v2")]
    if caller
        .data()
        .heap_access_v2()
        .resolve_handle(value::decode_handle(obj))
        .is_ok()
    {
        if value::is_array(obj) {
            crate::array_named_props::ArrayNamedPropsStore::set(caller, obj, name_id, val);
            return Some(());
        }
        let key = crate::property_key::canonicalize_v2_name_id(caller, name_id)?;
        return caller
            .data()
            .heap_access_v2()
            .define_data_property(value::decode_handle(obj), key, val as u64, flags as u32)
            .ok();
    }
    {
        // 数组实例：命名属性（含 .index/.input/.groups 等）存入宿主侧表，
        // 与编译期 obj_set 的数组分支一致；直接写对象堆会把数组元素存储当属性槽而损坏。
        if value::is_array(obj) {
            crate::array_named_props::ArrayNamedPropsStore::set(caller, obj, name_id, val);
            return Some(());
        }
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
        let slot_idx = num_props as usize;
        let slot_offset = actual_ptr + 16 + slot_idx * 32;
        {
            let data = env.memory.data_mut(&mut *caller);
            if slot_offset + 32 > data.len() {
                return None;
            }
            data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
            data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
        }
        let undef = value::encode_undefined();
        let handle = value::decode_object_handle(obj);
        crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle,
            slot_idx,
            crate::runtime_gc::heap_access::SlotPart::Value,
            val,
        )?;
        crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle,
            slot_idx,
            crate::runtime_gc::heap_access::SlotPart::Getter,
            undef,
        )?;
        crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle,
            slot_idx,
            crate::runtime_gc::heap_access::SlotPart::Setter,
            undef,
        )?;
        let data = env.memory.data_mut(&mut *caller);
        data[actual_ptr + 12..actual_ptr + 16].copy_from_slice(&(num_props + 1).to_le_bytes());
        Some(())
    }
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
    #[cfg(feature = "managed-heap-v2")]
    if caller
        .data()
        .heap_access_v2()
        .resolve_handle(value::decode_handle(obj))
        .is_ok()
    {
        // 与 define_data_property_on_array_named 一致：数组命名槽不承载访问器。
        if value::is_array(obj) {
            set_runtime_error(
                caller.data(),
                "TypeError: Accessor properties are not supported on array named slots"
                    .to_string(),
            );
            return None;
        }
        let key = crate::property_key::canonicalize_v2_name_id(caller, name_id)?;
        let flags = (attribute_flags & !constants::FLAG_WRITABLE) | constants::FLAG_IS_ACCESSOR;
        return caller
            .data()
            .heap_access_v2()
            .define_accessor_property_with_flags(
                value::decode_handle(obj),
                key,
                getter as u64,
                setter as u64,
                flags as u32,
            )
            .map_err(|error| {
                set_runtime_error(
                    caller.data(),
                    format!("V2 host accessor key {name_id}: {error}"),
                );
            })
            .ok();
    }
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
    let slot_idx = num_props as usize;
    let slot_offset = actual_ptr + 16 + slot_idx * 32;
    let flags = (attribute_flags & !constants::FLAG_WRITABLE) | constants::FLAG_IS_ACCESSOR;
    {
        let data = env.memory.data_mut(&mut *caller);
        if slot_offset + 32 > data.len() {
            return None;
        }
        data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
        data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    }
    let undef = value::encode_undefined();
    let handle = value::decode_object_handle(obj);
    crate::runtime_gc::heap_access::write_property_slot(
        caller,
        &env,
        handle,
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Value,
        undef,
    )?;
    crate::runtime_gc::heap_access::write_property_slot(
        caller,
        &env,
        handle,
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Getter,
        getter,
    )?;
    crate::runtime_gc::heap_access::write_property_slot(
        caller,
        &env,
        handle,
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Setter,
        setter,
    )?;
    let data = env.memory.data_mut(&mut *caller);
    data[actual_ptr + 12..actual_ptr + 16].copy_from_slice(&(num_props + 1).to_le_bytes());
    Some(())
}
