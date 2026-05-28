use anyhow::Result;
use wasmtime::{Caller, Linker, Func};
use wasmtime::Store;

use crate::*;

pub(crate) fn define_proxy_reflect(linker: &mut Linker<RuntimeState>, mut store: &mut Store<RuntimeState>) -> Result<()> {
    let proxy_create_fn = Func::wrap(&mut store,
        |caller: Caller<'_, RuntimeState>, target: i64, handler: i64| -> i64 {
            if !value::is_js_object(target) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Proxy target must be an object".to_string());
                return value::encode_undefined();
            }
            if !value::is_js_object(handler) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Proxy handler must be an object".to_string());
                return value::encode_undefined();
            }
            let handle;
            {
                let mut table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                handle = table.len() as u32;
                table.push(ProxyEntry { target, handler, revoked: false });
            }
            value::encode_proxy_handle(handle)
        },
    );
    linker.define(&mut store, "env", "proxy_create", proxy_create_fn)?;

    let reflect_get_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64, receiver: i64| -> i64 {
            // Proxy target: 触发 get trap
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'get' on a proxy that has been revoked".to_string());
                        return value::encode_undefined();
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "get")
                            .unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            return call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop, receiver])
                                .unwrap_or_else(|_| value::encode_undefined());
                        }
                    }
                    // 无 trap，转发到 target
                    return reflect_get_impl(&mut caller, entry.target, prop);
                }
                return value::encode_undefined();
            }
            reflect_get_impl(&mut caller, target, prop)
        },
    );
    linker.define(&mut store, "env", "reflect_get", reflect_get_fn)?;


    let proxy_revocable_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, handler: i64| -> i64 {
            if !value::is_js_object(target) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Proxy target must be an object".to_string());
                return value::encode_undefined();
            }
            if !value::is_js_object(handler) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Proxy handler must be an object".to_string());
                return value::encode_undefined();
            }
            let handle = {
                let mut table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                let handle = table.len() as u32;
                table.push(ProxyEntry { target, handler, revoked: false });
                handle
            };
            let proxy_val = value::encode_proxy_handle(handle);
            let revoke_fn = {
                let mut native_callables = caller.data().native_callables.lock().unwrap();
                let idx = native_callables.len() as u32;
                native_callables.push(NativeCallable::ProxyRevoker { proxy_handle: handle });
                value::encode_native_callable_idx(idx)
            };
            let obj = { let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv"); alloc_host_object(&mut caller, &_wjsm_env, 2) };
            let _ = define_host_data_property_from_caller(&mut caller, obj, "proxy", proxy_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "revoke", revoke_fn);
            obj
        },
    );
    linker.define(&mut store, "env", "proxy_revocable", proxy_revocable_fn)?;

    let reflect_set_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64, val: i64, receiver: i64| -> i64 {
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked { set_runtime_error(caller.data(), "TypeError: Cannot perform 'set' on a proxy that has been revoked".to_string()); return value::encode_bool(false); }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "set").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop, val, receiver]).unwrap_or_else(|_| value::encode_bool(false));
                            return value::encode_bool(nanbox_to_bool(result));
                        }
                    }
                    return reflect_set_impl(&mut caller, entry.target, prop, val);
                }
                return value::encode_bool(false);
            }
            reflect_set_impl(&mut caller, target, prop, val)
        },
    );
    linker.define(&mut store, "env", "reflect_set", reflect_set_fn)?;

    fn reflect_set_impl(caller: &mut Caller<'_, RuntimeState>, target: i64, prop: i64, val: i64) -> i64 {
        let Ok(prop_name) = render_value(caller, prop) else { return value::encode_bool(false); };
        let name_id = find_memory_c_string(caller, &prop_name);
        let existing = name_id.and_then(|id| {
            let obj_ptr = resolve_handle(caller, target)?;
            find_property_slot_by_name_id(caller, obj_ptr, id)
        });
        if let Some((_, flags, _)) = existing {
            let is_accessor = (flags & constants::FLAG_IS_ACCESSOR) != 0;
            if !is_accessor {
                let writable = (flags & constants::FLAG_WRITABLE) != 0;
                if !writable { return value::encode_bool(false); }
            }
        } else if !is_extensible_impl(caller, target) {
            return value::encode_bool(false);
        }
        let _ = define_host_data_property_from_caller(caller, target, &prop_name, val);
        value::encode_bool(true)
    }

    let reflect_has_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64| -> i64 {
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked { set_runtime_error(caller.data(), "TypeError: Cannot perform 'has' on a proxy that has been revoked".to_string()); return value::encode_bool(false); }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "has").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop]).unwrap_or_else(|_| value::encode_bool(false));
                            return value::encode_bool(nanbox_to_bool(result));
                        }
                    }
                    return reflect_has_impl(&mut caller, entry.target, prop);
                }
                return value::encode_bool(false);
            }
            reflect_has_impl(&mut caller, target, prop)
        },
    );
    linker.define(&mut store, "env", "reflect_has", reflect_has_fn)?;

    fn reflect_has_impl(caller: &mut Caller<'_, RuntimeState>, target: i64, prop: i64) -> i64 {
        let obj_ptr = resolve_handle(caller, target);
        if let Some(ptr) = obj_ptr
            && let Ok(prop_name) = render_value(caller, prop)
                && let Some(name_id) = find_memory_c_string(caller, &prop_name) {
                    let found = find_property_slot_by_name_id(caller, ptr, name_id).is_some();
                    return value::encode_bool(found);
                }
        value::encode_bool(false)
    }

    let reflect_delete_property_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64| -> i64 {
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked { set_runtime_error(caller.data(), "TypeError: Cannot perform 'deleteProperty' on a proxy that has been revoked".to_string()); return value::encode_bool(false); }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "deleteProperty").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop]).unwrap_or_else(|_| value::encode_bool(false));
                            return value::encode_bool(nanbox_to_bool(result));
                        }
                    }
                    return reflect_delete_property_impl(&mut caller, entry.target, prop);
                }
                return value::encode_bool(false);
            }
            reflect_delete_property_impl(&mut caller, target, prop)
        },
    );
    linker.define(&mut store, "env", "reflect_delete_property", reflect_delete_property_fn)?;
    fn reflect_delete_property_impl(caller: &mut Caller<'_, RuntimeState>, target: i64, prop: i64) -> i64 {
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
        let Some((
            slot_offset, flags, _val,
        )) = find_property_slot_by_name_id(caller, ptr, name_id)
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
        let num_props = u32::from_le_bytes([data[ptr + 12], data[ptr + 13], data[ptr + 14], data[ptr + 15]]) as usize;
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

    fn extract_array_like_elements(
        caller: &mut Caller<'_, RuntimeState>,
        arr_like: i64,
    ) -> Result<Vec<i64>, String> {
        let mut elements = Vec::new();
        if value::is_array(arr_like) {
            let handle = value::decode_array_handle(arr_like) as usize;
            let Some(ptr) = resolve_handle_idx(caller, handle) else { return Ok(elements); };
            let len = {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else { return Ok(elements); };
                let data = memory.data(&*caller);
                if ptr + 12 > data.len() { return Ok(elements); }
                u32::from_le_bytes([data[ptr + 8], data[ptr + 9], data[ptr + 10], data[ptr + 11]]) as usize
            };
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else { return Ok(elements); };
            for i in 0..len {
                let mut buf = [0u8; 8];
                if memory.read(&mut *caller, ptr + 16 + i * 8, &mut buf).is_ok() {
                    elements.push(i64::from_le_bytes(buf));
                }
            }
        } else if value::is_object(arr_like) || value::is_proxy(arr_like) {
            let len_prop = store_runtime_string(caller, "length".to_string());
            let len_val = reflect_get_impl(caller, arr_like, len_prop);
            let len = if value::is_f64(len_val) {
                value::decode_f64(len_val) as usize
            } else {
                0
            };
            for i in 0..len {
                let idx_prop = value::encode_f64(i as f64);
                let val = reflect_get_impl(caller, arr_like, idx_prop);
                elements.push(val);
            }
        }
        Ok(elements)
    }

    let reflect_apply_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, this_arg: i64, args_array: i64| -> i64 {
            if !is_callable_in_runtime(&mut caller, target) {
                set_runtime_error(caller.data(), "TypeError: Reflect.apply target must be callable".to_string());
                return value::encode_undefined();
            }
            let args = match extract_array_like_elements(&mut caller, args_array) {
                Ok(arr) => arr,
                Err(err) => {
                    set_runtime_error(caller.data(), err);
                    return value::encode_undefined();
                }
            };

            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().unwrap(); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'apply' on a proxy that has been revoked".to_string());
                        return value::encode_undefined();
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "apply").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let arr = alloc_array(&mut caller, args.len() as u32);
                            for (i, &arg) in args.iter().enumerate() {
                                set_array_elem(&mut caller, arr, i as i32, arg);
                            }
                            let trap_result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, this_arg, arr]);
                            return match trap_result {
                                Ok(res) => res,
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: Proxy apply trap failed: {}", e));
                                    value::encode_undefined()
                                }
                            };
                        }
                    }
                    return reflect_apply_impl(&mut caller, entry.target, this_arg, &args);
                }
            }

            reflect_apply_impl(&mut caller, target, this_arg, &args)
        },
    );
    linker.define(&mut store, "env", "reflect_apply", reflect_apply_fn)?;

    fn reflect_apply_impl(
        caller: &mut Caller<'_, RuntimeState>,
        target: i64,
        this_arg: i64,
        args: &[i64],
    ) -> i64 {
        let shadow_sp_global = caller.get_export("__shadow_sp").and_then(|e| e.into_global()).unwrap();
        let saved_sp = shadow_sp_global.get(&mut *caller).i32().unwrap();
        let memory = caller.get_export("memory").and_then(|e| e.into_memory()).unwrap();
        for (i, &arg) in args.iter().enumerate() {
            memory.write(&mut *caller, (saved_sp + i as i32 * 8) as usize, &arg.to_le_bytes()).unwrap();
        }
        shadow_sp_global.set(&mut *caller, Val::I32(saved_sp + args.len() as i32 * 8)).unwrap();
        let result = resolve_and_call(caller, target, this_arg, saved_sp, args.len() as i32);
        shadow_sp_global.set(&mut *caller, Val::I32(saved_sp)).unwrap();
        result
    }

    let reflect_construct_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, args_array: i64, new_target: i64| -> i64 {
            let n_target = if value::is_undefined(new_target) { target } else { new_target };
            if !is_callable_in_runtime(&mut caller, target) || !is_callable_in_runtime(&mut caller, n_target) {
                set_runtime_error(caller.data(), "TypeError: Reflect.construct target and newTarget must be constructors".to_string());
                return value::encode_undefined();
            }

            let args = match extract_array_like_elements(&mut caller, args_array) {
                Ok(arr) => arr,
                Err(err) => {
                    set_runtime_error(caller.data(), err);
                    return value::encode_undefined();
                }
            };

            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().unwrap(); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'construct' on a proxy that has been revoked".to_string());
                        return value::encode_undefined();
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "construct").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let arr = alloc_array(&mut caller, args.len() as u32);
                            for (i, &arg) in args.iter().enumerate() {
                                set_array_elem(&mut caller, arr, i as i32, arg);
                            }
                            let trap_result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, arr, n_target]);
                            return match trap_result {
                                Ok(res) => {
                                    if !value::is_js_object(res) {
                                        set_runtime_error(caller.data(), "TypeError: Proxy construct trap returned non-object".to_string());
                                        value::encode_undefined()
                                    } else {
                                        res
                                    }
                                }
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: Proxy construct trap failed: {}", e));
                                    value::encode_undefined()
                                }
                            };
                        }
                    }
                    return reflect_construct_impl(&mut caller, entry.target, &args, n_target);
                }
            }

            reflect_construct_impl(&mut caller, target, &args, n_target)
        },
    );
    linker.define(&mut store, "env", "reflect_construct", reflect_construct_fn)?;

    fn reflect_construct_impl(
        caller: &mut Caller<'_, RuntimeState>,
        target: i64,
        args: &[i64],
        new_target: i64,
    ) -> i64 {
        let this_obj = { let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv"); alloc_host_object(caller, &_wjsm_env, 4) };
        let proto_prop = store_runtime_string(caller, "prototype".to_string());
        let proto_val = reflect_get_impl(caller, new_target, proto_prop);
        if value::is_object(proto_val) || value::is_array(proto_val) || value::is_proxy(proto_val) || value::is_null(proto_val) {
            let _ = reflect_set_prototype_of_fn_impl(caller, this_obj, proto_val);
        }

        let shadow_sp_global = caller.get_export("__shadow_sp").and_then(|e| e.into_global()).expect("__shadow_sp in reflect_construct_impl");
        let saved_sp = shadow_sp_global.get(&mut *caller).i32().expect("shadow_sp i32 in reflect_construct_impl");
        let memory = caller.get_export("memory").and_then(|e| e.into_memory()).expect("memory in reflect_construct_impl");
        for (i, &arg) in args.iter().enumerate() {
            memory.write(&mut *caller, (saved_sp + i as i32 * 8) as usize, &arg.to_le_bytes()).expect("shadow stack write in reflect_construct_impl");
        }
        shadow_sp_global.set(&mut *caller, Val::I32(saved_sp + args.len() as i32 * 8)).expect("shadow_sp set in reflect_construct_impl");
        let result = resolve_and_call(caller, target, this_obj, saved_sp, args.len() as i32);
        shadow_sp_global.set(&mut *caller, Val::I32(saved_sp)).expect("shadow_sp restore in reflect_construct_impl");

        if value::is_js_object(result) {
            result
        } else {
            this_obj
        }
    }

    let reflect_get_prototype_of_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64| -> i64 {
            if !value::is_object(target) && !value::is_array(target) && !value::is_function(target) && !value::is_proxy(target) {
                set_runtime_error(caller.data(), "TypeError: Reflect.getPrototypeOf called on non-object".to_string());
                return value::encode_undefined();
            }
            reflect_get_prototype_of_impl(&mut caller, target)
        },
    );
    linker.define(&mut store, "env", "reflect_get_prototype_of", reflect_get_prototype_of_fn)?;

    fn reflect_get_prototype_of_impl(caller: &mut Caller<'_, RuntimeState>, target: i64) -> i64 {
        if value::is_proxy(target) {
            let handle = value::decode_proxy_handle(target) as usize;
            let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
            if let Some(entry) = entry {
                if entry.revoked { set_runtime_error(caller.data(), "TypeError: Cannot perform 'getPrototypeOf' on a proxy that has been revoked".to_string()); return value::encode_undefined(); }
                if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                    let trap = read_object_property_by_name(caller, handler_ptr, "getPrototypeOf").unwrap_or_else(value::encode_undefined);
                    if !value::is_undefined(trap) && !value::is_null(trap) {
                        let res = call_wasm_callback(caller, trap, entry.handler, &[entry.target]).unwrap_or_else(|_| value::encode_null());
                        // 不变量检查: getPrototypeOf trap 返回值必须是 null 或对象
                        if !value::is_null(res) && !value::is_object(res) && !value::is_array(res) && !value::is_proxy(res) && !value::is_function(res) {
                            set_runtime_error(caller.data(), "TypeError: Proxy getPrototypeOf must return an object or null".to_string());
                            return value::encode_null();
                        }
                        // 不变量检查: 若 target 不可扩展，返回的原型必须与 target 原型一致
                        let ext = is_extensible_impl(caller, entry.target);
                        if !ext {
                            let target_proto = reflect_get_prototype_of_impl(caller, entry.target);
                            if res != target_proto {
                                set_runtime_error(caller.data(), "TypeError: Proxy getPrototypeOf invariant violated: target is not extensible and trap returned different prototype".to_string());
                                return value::encode_null();
                            }
                        }
                        return res;
                    }
                }
                return reflect_get_prototype_of_impl(caller, entry.target);
            }
        }
        let Some(ptr) = resolve_handle(caller, target) else { return value::encode_null(); };
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else { return value::encode_null(); };
        let data = memory.data(&*caller);
        if ptr + 4 > data.len() { return value::encode_null(); }
        let proto_handle = u32::from_le_bytes([data[ptr], data[ptr + 1], data[ptr + 2], data[ptr + 3]]);
        if proto_handle == 0xFFFF_FFFF { value::encode_null() } else { value::encode_object_handle(proto_handle) }
    }

    fn is_prototype_circular_chain(
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
            if value::is_proxy(current) {
                let handle = value::decode_proxy_handle(current);
                if !visited.insert(handle) {
                    break;
                }
            } else if value::is_object(current) {
                let handle = value::decode_object_handle(current);
                if !visited.insert(handle) {
                    break;
                }
            } else if value::is_array(current) {
                let handle = value::decode_array_handle(current);
                if !visited.insert(handle) {
                    break;
                }
            }
            current = reflect_get_prototype_of_impl(caller, current);
        }
        false
    }

    fn reflect_set_prototype_of_fn_impl(
        caller: &mut Caller<'_, RuntimeState>,
        target: i64,
        proto: i64,
    ) -> i64 {
        if !is_extensible_impl(caller, target) {
            let current_proto = reflect_get_prototype_of_impl(caller, target);
            return value::encode_bool(current_proto == proto);
        }
        if is_prototype_circular_chain(caller, target, proto) {
            return value::encode_bool(false);
        }
        let Some(ptr) = resolve_handle(caller, target) else { return value::encode_bool(false); };
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else { return value::encode_bool(false); };

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
        if ptr + 4 > data.len() { return value::encode_bool(false); }
        data[ptr..ptr + 4].copy_from_slice(&proto_handle.to_le_bytes());
        value::encode_bool(true)
    }

    let reflect_set_prototype_of_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, proto: i64| -> i64 {
            if !value::is_object(target) && !value::is_array(target) && !value::is_function(target) && !value::is_proxy(target) {
                set_runtime_error(caller.data(), "TypeError: Reflect.setPrototypeOf called on non-object".to_string());
                return value::encode_bool(false);
            }
            if !value::is_object(proto) && !value::is_null(proto) && !value::is_proxy(proto) && !value::is_array(proto) && !value::is_function(proto) {
                set_runtime_error(caller.data(), "TypeError: Reflect.setPrototypeOf prototype must be an object or null".to_string());
                return value::encode_bool(false);
            }

            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().unwrap(); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'setPrototypeOf' on a proxy that has been revoked".to_string());
                        return value::encode_bool(false);
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "setPrototypeOf").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, proto]);
                            let trap_res = match result {
                                Ok(res) => !value::is_falsy(res),
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: setPrototypeOf trap failed: {}", e));
                                    return value::encode_bool(false);
                                }
                            };
                            if trap_res {
                                let ext = is_extensible_impl(&mut caller, entry.target);
                                if !ext {
                                    let current_proto = reflect_get_prototype_of_impl(&mut caller, entry.target);
                                    if current_proto != proto {
                                        set_runtime_error(caller.data(), "TypeError: Proxy setPrototypeOf invariant violated: target is not extensible and new prototype is different".to_string());
                                        return value::encode_bool(false);
                                    }
                                }
                            }
                            return value::encode_bool(trap_res);
                        }
                    }
                    return reflect_set_prototype_of_fn_impl(&mut caller, entry.target, proto);
                }
            }
            reflect_set_prototype_of_fn_impl(&mut caller, target, proto)
        },
    );
    linker.define(&mut store, "env", "reflect_set_prototype_of", reflect_set_prototype_of_fn)?;

    let reflect_is_extensible_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64| -> i64 {
            if !value::is_object(target) && !value::is_array(target) && !value::is_function(target) && !value::is_proxy(target) {
                set_runtime_error(caller.data(), "TypeError: Reflect.isExtensible called on non-object".to_string());
                return value::encode_bool(false);
            }
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().unwrap();
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'isExtensible' on a proxy that has been revoked".to_string());
                        return value::encode_bool(false);
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "isExtensible")
                            .unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target]);
                            let trap_res = match result {
                                Ok(res) => !value::is_falsy(res),
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: isExtensible trap failed: {}", e));
                                    return value::encode_bool(false);
                                }
                            };
                            let real_res = is_extensible_impl(&mut caller, entry.target);
                            if trap_res != real_res {
                                set_runtime_error(caller.data(), "TypeError: Proxy isExtensible trap returned result that does not match target's extensibility".to_string());
                                return value::encode_bool(false);
                            }
                            return value::encode_bool(trap_res);
                        }
                    }
                    return value::encode_bool(is_extensible_impl(&mut caller, entry.target));
                }
            }
            value::encode_bool(is_extensible_impl(&mut caller, target))
        },
    );
    linker.define(&mut store, "env", "reflect_is_extensible", reflect_is_extensible_fn)?;

    let reflect_prevent_extensions_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64| -> i64 {
            if !value::is_object(target) && !value::is_array(target) && !value::is_function(target) && !value::is_proxy(target) {
                set_runtime_error(caller.data(), "TypeError: Reflect.preventExtensions called on non-object".to_string());
                return value::encode_bool(false);
            }
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().unwrap();
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'preventExtensions' on a proxy that has been revoked".to_string());
                        return value::encode_bool(false);
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "preventExtensions")
                            .unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target]);
                            let trap_res = match result {
                                Ok(res) => !value::is_falsy(res),
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: preventExtensions trap failed: {}", e));
                                    return value::encode_bool(false);
                                }
                            };
                            if trap_res {
                                let real_res = is_extensible_impl(&mut caller, entry.target);
                                if real_res {
                                    set_runtime_error(caller.data(), "TypeError: Proxy preventExtensions trap returned true, but target remains extensible".to_string());
                                    return value::encode_bool(false);
                                }
                            }
                            return value::encode_bool(trap_res);
                        }
                    }
                    return value::encode_bool(prevent_extensions_impl(&mut caller, entry.target));
                }
            }
            value::encode_bool(prevent_extensions_impl(&mut caller, target))
        },
    );
    linker.define(&mut store, "env", "reflect_prevent_extensions", reflect_prevent_extensions_fn)?;

    let reflect_get_own_property_descriptor_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64| -> i64 {
            // Proxy target: trigger getOwnPropertyDescriptor trap
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked { set_runtime_error(caller.data(), "TypeError: Cannot perform 'getOwnPropertyDescriptor' on a proxy that has been revoked".to_string()); return value::encode_undefined(); }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "getOwnPropertyDescriptor").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let trap_result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop]);
                            let descriptor = match trap_result {
                                Ok(desc) => desc,
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: getOwnPropertyDescriptor trap failed: {}", e));
                                    return value::encode_undefined();
                                }
                            };
                            if value::is_undefined(descriptor) {
                                let prop_name = render_value(&mut caller, prop).ok();
                                if let Some(name) = prop_name {
                                    if let Some(name_id) = find_memory_c_string(&mut caller, &name) {
                                        if let Some(t_ptr) = resolve_handle(&mut caller, entry.target) {
                                            if let Some((_, flags, _)) = find_property_slot_by_name_id(&mut caller, t_ptr, name_id) {
                                                let configurable = (flags & constants::FLAG_CONFIGURABLE) != 0;
                                                if !configurable {
                                                    set_runtime_error(caller.data(), "TypeError: Proxy getOwnPropertyDescriptor invariant violated: non-configurable property must not be reported as undefined".to_string());
                                                    return value::encode_undefined();
                                                }
                                            } else if !is_extensible_impl(&mut caller, entry.target) {
                                                set_runtime_error(caller.data(), "TypeError: Proxy getOwnPropertyDescriptor invariant violated: target is non-extensible and property exists".to_string());
                                                return value::encode_undefined();
                                            }
                                        }
                                    }
                                }
                            }
                            return descriptor;
                        }
                    }
                    return reflect_get_own_property_descriptor_impl(&mut caller, entry.target, prop);
                }
                return value::encode_undefined();
            }
            reflect_get_own_property_descriptor_impl(&mut caller, target, prop)
        },
    );
    linker.define(&mut store, "env", "reflect_get_own_property_descriptor", reflect_get_own_property_descriptor_fn)?;
    fn reflect_get_own_property_descriptor_impl(caller: &mut Caller<'_, RuntimeState>, target: i64, prop: i64) -> i64 {
        let prop_name = match render_value(caller, prop) {
            Ok(name) => name,
            Err(_) => return value::encode_undefined(),
        };
        let Some(ptr) = resolve_handle(caller, target) else {
            return value::encode_undefined();
        };
        let Some(name_id) = find_memory_c_string(caller, &prop_name) else {
            return value::encode_undefined();
        };
        let Some((slot_offset, flags, val)) = find_property_slot_by_name_id(caller, ptr, name_id) else {
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
            if slot_offset + 32 > data.len() { return value::encode_undefined(); }
            let g = i64::from_le_bytes([
                data[slot_offset + 16], data[slot_offset + 17], data[slot_offset + 18], data[slot_offset + 19],
                data[slot_offset + 20], data[slot_offset + 21], data[slot_offset + 22], data[slot_offset + 23],
            ]);
            let s = i64::from_le_bytes([
                data[slot_offset + 24], data[slot_offset + 25], data[slot_offset + 26], data[slot_offset + 27],
                data[slot_offset + 28], data[slot_offset + 29], data[slot_offset + 30], data[slot_offset + 31],
            ]);
            (g, s)
        } else {
            (value::encode_undefined(), value::encode_undefined())
        };
        let desc = { let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv"); alloc_host_object(caller, &_wjsm_env, 4) };
        if is_accessor {
            let _ = define_host_data_property_from_caller(caller, desc, "get", getter_val);
            let _ = define_host_data_property_from_caller(caller, desc, "set", setter_val);
        } else {
            let _ = define_host_data_property_from_caller(caller, desc, "value", val);
            let _ = define_host_data_property_from_caller(caller, desc, "writable", value::encode_bool((flags & constants::FLAG_WRITABLE) != 0));
        }
        let _ = define_host_data_property_from_caller(caller, desc, "enumerable", value::encode_bool(enumerable));
        let _ = define_host_data_property_from_caller(caller, desc, "configurable", value::encode_bool(configurable));
        desc
    }

    let reflect_define_property_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64, descriptor: i64| -> i64 {
            match define_property_internal(&mut caller, target, prop, descriptor) {
                Ok(success) => value::encode_bool(success),
                Err(e) => {
                    set_runtime_error(caller.data(), e);
                    value::encode_bool(false)
                }
            }
        },
    );
    linker.define(&mut store, "env", "reflect_define_property", reflect_define_property_fn)?;

    fn reflect_own_keys_impl(caller: &mut Caller<'_, RuntimeState>, target: i64) -> i64 {
        let Some(ptr) = resolve_handle(caller, target) else { return value::encode_undefined(); };
        let names = collect_own_property_names(caller, ptr, false);
        let arr = alloc_array(caller, names.len() as u32);
        for (i, name) in names.into_iter().enumerate() {
            let name_val = store_runtime_string(caller, name);
            set_array_elem(caller, arr, i as i32, name_val);
        }
        arr
    }

    let reflect_own_keys_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64| -> i64 {
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'ownKeys' on a proxy that has been revoked".to_string());
                        return value::encode_undefined();
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "ownKeys")
                            .unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let trap_res = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target]);
                            let keys_val = match trap_res {
                                Ok(res) => res,
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: Proxy ownKeys trap failed: {}", e));
                                    return value::encode_undefined();
                                }
                            };
                            let keys = match extract_array_like_elements(&mut caller, keys_val) {
                                Ok(arr) => arr,
                                Err(err) => {
                                    set_runtime_error(caller.data(), err);
                                    return value::encode_undefined();
                                }
                            };

                            // Invariant checks
                            let ext = is_extensible_impl(&mut caller, entry.target);
                            let Some(t_ptr) = resolve_handle(&mut caller, entry.target) else { return value::encode_undefined(); };
                            let target_keys = collect_own_property_names(&mut caller, t_ptr, false);
                            let mut trap_keys_str = Vec::new();
                            for &k in &keys {
                                // 跳过 Symbol 键（不出现在 collect_own_property_names 的结果中）
                                if value::is_symbol(k) { continue; }
                                if let Ok(k_str) = render_value(&mut caller, k) {
                                    trap_keys_str.push(k_str);
                                }
                            }

                            if !ext {
                                let mut match_all = true;
                                for tk in &target_keys {
                                    if !trap_keys_str.contains(tk) {
                                        match_all = false;
                                        break;
                                    }
                                }
                                if !match_all || trap_keys_str.len() != target_keys.len() {
                                    set_runtime_error(caller.data(), "TypeError: Proxy ownKeys invariant violated: target is non-extensible and keys do not match target keys".to_string());
                                    return value::encode_undefined();
                                }
                            } else {
                                for tk in &target_keys {
                                    if let Some(tk_c) = find_memory_c_string(&mut caller, tk) {
                                        if let Some((_, flags, _)) = find_property_slot_by_name_id(&mut caller, t_ptr, tk_c) {
                                            let configurable = (flags & constants::FLAG_CONFIGURABLE) != 0;
                                            if !configurable && !trap_keys_str.contains(tk) {
                                                set_runtime_error(caller.data(), format!("TypeError: Proxy ownKeys invariant violated: non-configurable property '{}' is missing in trap result", tk));
                                                return value::encode_undefined();
                                            }
                                        }
                                    }
                                }
                            }

                            let arr = alloc_array(&mut caller, keys.len() as u32);
                            for (i, &key) in keys.iter().enumerate() {
                                set_array_elem(&mut caller, arr, i as i32, key);
                            }
                            return arr;
                        }
                    }
                    return reflect_own_keys_impl(&mut caller, entry.target);
                }
            }
            reflect_own_keys_impl(&mut caller, target)
        },
    );
    linker.define(&mut store, "env", "reflect_own_keys", reflect_own_keys_fn)?;

    // ── proxy_apply: WASM 调用路径中 TAG_PROXY 的 [[Call]] 派发 ──
    let proxy_apply_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, proxy: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let handle = value::decode_proxy_handle(proxy) as usize;
            let entry = {
                let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                table.get(handle).cloned()
            };
            if let Some(entry) = entry {
                if entry.revoked {
                    set_runtime_error(caller.data(), "TypeError: Cannot perform call on a proxy that has been revoked".to_string());
                    return value::encode_undefined();
                }
                // 从影子栈读取参数
                let args: Vec<i64> = (0..args_count.max(0))
                    .map(|i| read_shadow_arg(&mut caller, args_base, i as u32))
                    .collect();
                // 查找 handler 的 apply trap
                if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                    let trap = read_object_property_by_name(&mut caller, handler_ptr, "apply")
                        .unwrap_or_else(value::encode_undefined);
                    if !value::is_undefined(trap) && !value::is_null(trap) {
                        let arr = alloc_array(&mut caller, args.len() as u32);
                        for (i, &arg) in args.iter().enumerate() {
                            set_array_elem(&mut caller, arr, i as i32, arg);
                        }
                        return call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, this_val, arr])
                            .unwrap_or_else(|_| {
                                set_runtime_error(caller.data(), "TypeError: Proxy apply trap failed".to_string());
                                value::encode_undefined()
                            });
                    }
                }
                // 无 trap，转发到 target
                return reflect_apply_impl(&mut caller, entry.target, this_val, &args);
            }
            value::encode_undefined()
        },
    );
    linker.define(&mut store, "env", "proxy.apply", proxy_apply_fn)?;

    // ── proxy_construct: WASM 调用路径中 TAG_PROXY 的 [[Construct]] 派发 ──
    let proxy_construct_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, proxy: i64, _this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let handle = value::decode_proxy_handle(proxy) as usize;
            let entry = {
                let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                table.get(handle).cloned()
            };
            if let Some(entry) = entry {
                if entry.revoked {
                    set_runtime_error(caller.data(), "TypeError: Cannot perform construct on a proxy that has been revoked".to_string());
                    return value::encode_undefined();
                }
                // 从影子栈读取参数
                let args: Vec<i64> = (0..args_count.max(0))
                    .map(|i| read_shadow_arg(&mut caller, args_base, i as u32))
                    .collect();
                // 查找 handler 的 construct trap
                if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                    let trap = read_object_property_by_name(&mut caller, handler_ptr, "construct")
                        .unwrap_or_else(value::encode_undefined);
                    if !value::is_undefined(trap) && !value::is_null(trap) {
                        // 不变量检查: target 必须是可调用的函数
                        if !is_callable_in_runtime(&mut caller, entry.target) {
                            set_runtime_error(caller.data(), "TypeError: Proxy construct: target must be a callable function".to_string());
                            return value::encode_undefined();
                        }
                        let arr = alloc_array(&mut caller, args.len() as u32);
                        for (i, &arg) in args.iter().enumerate() {
                            set_array_elem(&mut caller, arr, i as i32, arg);
                        }
                        let trap_result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, arr, proxy]);
                        return match trap_result {
                            Ok(res) => {
                                if !value::is_js_object(res) {
                                    set_runtime_error(caller.data(), "TypeError: Proxy construct trap returned non-object".to_string());
                                    value::encode_undefined()
                                } else {
                                    res
                                }
                            }
                            Err(e) => {
                                set_runtime_error(caller.data(), format!("TypeError: Proxy construct trap failed: {}", e));
                                value::encode_undefined()
                            }
                        };
                    }
                }
                // 无 trap，转发到 target
                return reflect_construct_impl(&mut caller, entry.target, &args, proxy);
            }
            value::encode_undefined()
        },
    );
    linker.define(&mut store, "env", "proxy.construct", proxy_construct_fn)?;

    Ok(())
}
