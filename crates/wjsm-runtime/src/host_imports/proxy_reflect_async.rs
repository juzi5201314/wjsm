//! Async overrides for `define_proxy_reflect` reentrant host imports.

use anyhow::Result;
use wasmtime::{Caller, Linker};

use super::proxy_reflect::{
    check_proxy_revoked, extract_array_like_elements, proxy_own_keys_trap_async,
    reflect_apply_impl_async, reflect_construct_impl_async, reflect_delete_property_impl,
    reflect_get_own_property_descriptor_impl, reflect_get_prototype_of_async, reflect_has_impl,
    reflect_own_keys_impl, reflect_set_impl, reflect_set_prototype_of_fn_impl,
};
use super::proxy_traps::proxy_is_revoked;
use crate::*;

pub(crate) fn define_proxy_reflect_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "reflect_get",
        |mut caller: Caller<'_, RuntimeState>, (target, prop, receiver): (i64, i64, i64)| {
            Box::new(async move {
                reflect_get_impl_with_receiver_async(&mut caller, target, prop, receiver).await
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "reflect_set",
        |mut caller: Caller<'_, RuntimeState>,
         (target, prop, val, receiver): (i64, i64, i64, i64)| {
            Box::new(async move {
                if value::is_proxy(target) {
                    let handle = value::decode_proxy_handle(target) as usize;
                    let entry = {
                        let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                        table.get(handle).cloned()
                    };
                    if let Some(entry) = entry {
                        if let Some(exc) = check_proxy_revoked(&mut caller, &entry, "set") {
                            return exc;
                        }
                        if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                            let trap =
                                read_object_property_by_name(&mut caller, handler_ptr, "set")
                                    .unwrap_or_else(value::encode_undefined);
                            if !value::is_undefined(trap) && !value::is_null(trap) {
                                let result = call_wasm_callback_async(
                                    &mut caller,
                                    trap,
                                    entry.handler,
                                    &[entry.target, prop, val, receiver],
                                )
                                .await
                                .unwrap_or_else(|_| value::encode_bool(false));
                                return value::encode_bool(nanbox_to_bool(result));
                            }
                        }
                        return reflect_set_impl(&mut caller, entry.target, prop, val);
                    }
                    return value::encode_bool(false);
                }
                reflect_set_impl(&mut caller, target, prop, val)
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "reflect_has",
        |mut caller: Caller<'_, RuntimeState>, (target, prop): (i64, i64)| {
            Box::new(async move {
                if value::is_proxy(target) {
                    let handle = value::decode_proxy_handle(target) as usize;
                    let entry = {
                        let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                        table.get(handle).cloned()
                    };
                    if let Some(entry) = entry {
                        if let Some(exc) = check_proxy_revoked(&mut caller, &entry, "has") {
                            return exc;
                        }
                        if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                            let trap =
                                read_object_property_by_name(&mut caller, handler_ptr, "has")
                                    .unwrap_or_else(value::encode_undefined);
                            if !value::is_undefined(trap) && !value::is_null(trap) {
                                let result = call_wasm_callback_async(
                                    &mut caller,
                                    trap,
                                    entry.handler,
                                    &[entry.target, prop],
                                )
                                .await
                                .unwrap_or_else(|_| value::encode_bool(false));
                                return value::encode_bool(nanbox_to_bool(result));
                            }
                        }
                        return reflect_has_impl(&mut caller, entry.target, prop);
                    }
                    return value::encode_bool(false);
                }
                reflect_has_impl(&mut caller, target, prop)
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "reflect_delete_property",
        |mut caller: Caller<'_, RuntimeState>, (target, prop): (i64, i64)| {
            Box::new(async move {
                if value::is_proxy(target) {
                    let handle = value::decode_proxy_handle(target) as usize;
                    let entry = {
                        let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                        table.get(handle).cloned()
                    };
                    if let Some(entry) = entry {
                        if let Some(exc) =
                            check_proxy_revoked(&mut caller, &entry, "deleteProperty")
                        {
                            return exc;
                        }
                        if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                            let trap = read_object_property_by_name(
                                &mut caller,
                                handler_ptr,
                                "deleteProperty",
                            )
                            .unwrap_or_else(value::encode_undefined);
                            if !value::is_undefined(trap) && !value::is_null(trap) {
                                let result = call_wasm_callback_async(
                                    &mut caller,
                                    trap,
                                    entry.handler,
                                    &[entry.target, prop],
                                )
                                .await
                                .unwrap_or_else(|_| value::encode_bool(false));
                                return value::encode_bool(nanbox_to_bool(result));
                            }
                        }
                        return reflect_delete_property_impl(&mut caller, entry.target, prop);
                    }
                    return value::encode_bool(false);
                }
                reflect_delete_property_impl(&mut caller, target, prop)
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "reflect_apply",
        |mut caller: Caller<'_, RuntimeState>, (target, this_arg, args_array): (i64, i64, i64)| {
            Box::new(async move {
                if !is_callable_in_runtime(&mut caller, target) {
                    return make_type_error_exception(
                        &mut caller,
                        "TypeError: Reflect.apply target must be callable",
                    );
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
                    let entry = {
                        let table = caller.data().proxy_table.lock().unwrap();
                        table.get(handle).cloned()
                    };
                    if let Some(entry) = entry {
                        if let Some(exc) = check_proxy_revoked(&mut caller, &entry, "apply") {
                            return exc;
                        }
                        if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                            let trap =
                                read_object_property_by_name(&mut caller, handler_ptr, "apply")
                                    .unwrap_or_else(value::encode_undefined);
                            if !value::is_undefined(trap) && !value::is_null(trap) {
                                let arr = alloc_array(&mut caller, args.len() as u32);
                                for (i, &arg) in args.iter().enumerate() {
                                    set_array_elem(&mut caller, arr, i as i32, arg);
                                }
                                return match call_wasm_callback_async(
                                    &mut caller,
                                    trap,
                                    entry.handler,
                                    &[entry.target, this_arg, arr],
                                )
                                .await
                                {
                                    Ok(res) => res,
                                    Err(e) => {
                                        set_runtime_error(
                                            caller.data(),
                                            format!("TypeError: Proxy apply trap failed: {}", e),
                                        );
                                        value::encode_undefined()
                                    }
                                };
                            }
                        }
                        return reflect_apply_impl_async(
                            &mut caller,
                            entry.target,
                            this_arg,
                            &args,
                        )
                        .await;
                    }
                }
                reflect_apply_impl_async(&mut caller, target, this_arg, &args).await
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "reflect_construct",
        |mut caller: Caller<'_, RuntimeState>, (target, args_array, new_target): (i64, i64, i64)| {
            Box::new(async move {
                let n_target = if value::is_undefined(new_target) { target } else { new_target };
                if !is_constructor_in_runtime(&mut caller, target) || !is_constructor_in_runtime(&mut caller, n_target) {
                    return make_type_error_exception(&mut caller, "TypeError: Reflect.construct target and newTarget must be constructors");
                }
                let args = match extract_array_like_elements(&mut caller, args_array) {
                    Ok(arr) => arr,
                    Err(err) => { set_runtime_error(caller.data(), err); return value::encode_undefined(); }
                };
                if value::is_proxy(target) {
                    let handle = value::decode_proxy_handle(target) as usize;
                    let entry = { let table = caller.data().proxy_table.lock().unwrap(); table.get(handle).cloned() };
                    if let Some(entry) = entry {
                        if let Some(exc) = check_proxy_revoked(&mut caller, &entry, "construct") {
                            return exc;
                        }
                        if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                            let trap = read_object_property_by_name(&mut caller, handler_ptr, "construct").unwrap_or_else(value::encode_undefined);
                            if !value::is_undefined(trap) && !value::is_null(trap) {
                                let arr = alloc_array(&mut caller, args.len() as u32);
                                for (i, &arg) in args.iter().enumerate() { set_array_elem(&mut caller, arr, i as i32, arg); }
                                return match call_wasm_callback_async(&mut caller, trap, entry.handler, &[entry.target, arr, n_target]).await {
                                    Ok(res) => {
                                        if !value::is_js_object(res) {
                                            make_type_error_exception(&mut caller, "TypeError: Proxy construct trap returned non-object")
                                        } else { res }
                                    }
                                    Err(e) => { set_runtime_error(caller.data(), format!("TypeError: Proxy construct trap failed: {}", e)); value::encode_undefined() }
                                };
                            }
                        }
                        return reflect_construct_impl_async(&mut caller, entry.target, &args, n_target).await;
                    }
                }
                reflect_construct_impl_async(&mut caller, target, &args, n_target).await
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "reflect_get_prototype_of",
        |mut caller: Caller<'_, RuntimeState>, (target,): (i64,)| {
            Box::new(async move {
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
                reflect_get_prototype_of_async(&mut caller, target).await
            })
        },
    )?;

    linker.func_wrap_async("env", "reflect_set_prototype_of", |mut caller: Caller<'_, RuntimeState>, (target, proto): (i64, i64)| {
        Box::new(async move {
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
                    if let Some(exc) = check_proxy_revoked(&mut caller, &entry, "setPrototypeOf") {
                        return exc;
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "setPrototypeOf").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback_async(&mut caller, trap, entry.handler, &[entry.target, proto]).await;
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
                                    let current_proto = reflect_get_prototype_of_async(&mut caller, entry.target).await;
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
        })
    })?;

    linker.func_wrap_async(
        "env",
        "reflect_is_extensible",
        |mut caller: Caller<'_, RuntimeState>, (target,): (i64,)| {
            Box::new(async move {
                if !value::is_object(target)
                    && !value::is_array(target)
                    && !value::is_function(target)
                    && !value::is_proxy(target)
                {
                    set_runtime_error(
                        caller.data(),
                        "TypeError: Reflect.isExtensible called on non-object".to_string(),
                    );
                    return value::encode_bool(false);
                }
                if proxy_is_revoked(&mut caller, target) {
                    return make_type_error_exception(
                        &mut caller,
                        "TypeError: Cannot perform 'isExtensible' on a proxy that has been revoked",
                    );
                }
                value::encode_bool(
                    proxy_or_target_is_extensible_impl_async(&mut caller, target).await,
                )
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "reflect_prevent_extensions",
        |mut caller: Caller<'_, RuntimeState>, (target,): (i64,)| {
            Box::new(async move {
                if !value::is_object(target)
                    && !value::is_array(target)
                    && !value::is_function(target)
                    && !value::is_proxy(target)
                {
                    set_runtime_error(
                        caller.data(),
                        "TypeError: Reflect.preventExtensions called on non-object".to_string(),
                    );
                    return value::encode_bool(false);
                }
                if proxy_is_revoked(&mut caller, target) {
                    return make_type_error_exception(&mut caller, "TypeError: Cannot perform 'preventExtensions' on a proxy that has been revoked");
                }
                value::encode_bool(
                    proxy_or_target_prevent_extensions_impl_async(&mut caller, target).await,
                )
            })
        },
    )?;

    linker.func_wrap_async("env", "reflect_get_own_property_descriptor", |mut caller: Caller<'_, RuntimeState>, (target, prop): (i64, i64)| {
        Box::new(async move {
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if let Some(exc) = check_proxy_revoked(&mut caller, &entry, "getOwnPropertyDescriptor") {
                        return exc;
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "getOwnPropertyDescriptor").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let descriptor = match call_wasm_callback_async(&mut caller, trap, entry.handler, &[entry.target, prop]).await {
                                Ok(desc) => desc,
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: getOwnPropertyDescriptor trap failed: {}", e));
                                    return value::encode_undefined();
                                }
                            };
                            let prop_name = render_value(&mut caller, prop).ok();
                            let name_id = prop_name.as_deref().and_then(|name| find_memory_c_string(&mut caller, name));
                            if let Err(error) = validate_proxy_get_own_property_descriptor_result(&mut caller, entry.target, name_id, descriptor) {
                                set_runtime_error(caller.data(), error);
                                return value::encode_undefined();
                            }
                            return descriptor;
                        }
                    }
                    return reflect_get_own_property_descriptor_impl(&mut caller, entry.target, prop);
                }
                return value::encode_undefined();
            }
            reflect_get_own_property_descriptor_impl(&mut caller, target, prop)
        })
    })?;

    linker.func_wrap_async(
        "env",
        "reflect_define_property",
        |mut caller: Caller<'_, RuntimeState>, (target, prop, descriptor): (i64, i64, i64)| {
            Box::new(async move {
                if proxy_is_revoked(&mut caller, target) {
                    return make_type_error_exception(&mut caller, "TypeError: Cannot perform 'defineProperty' on a proxy that has been revoked");
                }
                match define_property_internal_async(&mut caller, target, prop, descriptor).await {
                    Ok(success) => value::encode_bool(success),
                    Err(e) => {
                        set_runtime_error(caller.data(), e);
                        value::encode_bool(false)
                    }
                }
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "reflect_own_keys",
        |mut caller: Caller<'_, RuntimeState>, (target,): (i64,)| {
            Box::new(async move {
                if value::is_proxy(target) {
                    let res = proxy_own_keys_trap_async(&mut caller, target).await;
                    if !value::is_undefined(res) {
                        return res;
                    }
                    if caller
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime error mutex")
                        .is_some()
                    {
                        return value::encode_undefined();
                    }
                    let handle = value::decode_proxy_handle(target) as usize;
                    let entry = caller
                        .data()
                        .proxy_table
                        .lock()
                        .expect("proxy_table mutex")
                        .get(handle)
                        .cloned();
                    if let Some(entry) = entry {
                        return reflect_own_keys_impl(&mut caller, entry.target);
                    }
                    return value::encode_undefined();
                }
                reflect_own_keys_impl(&mut caller, target)
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "proxy.apply",
        |mut caller: Caller<'_, RuntimeState>,
         (proxy, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let handle = value::decode_proxy_handle(proxy) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if let Some(_exc) = check_proxy_revoked(&mut caller, &entry, "call") {
                        set_runtime_error(
                            caller.data(),
                            "TypeError: Cannot perform call on a proxy that has been revoked"
                                .to_string(),
                        );
                        return value::encode_undefined();
                    }
                    if !is_callable_in_runtime(&mut caller, entry.target) {
                        set_runtime_error(
                            caller.data(),
                            "TypeError: Proxy target must be callable".to_string(),
                        );
                        return value::encode_undefined();
                    }
                    let args: Vec<i64> = (0..args_count.max(0))
                        .map(|i| read_shadow_arg(&mut caller, args_base, i as u32))
                        .collect();
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "apply")
                            .unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let arr = alloc_array(&mut caller, args.len() as u32);
                            for (i, &arg) in args.iter().enumerate() {
                                set_array_elem(&mut caller, arr, i as i32, arg);
                            }
                            return call_wasm_callback_async(
                                &mut caller,
                                trap,
                                entry.handler,
                                &[entry.target, this_val, arr],
                            )
                            .await
                            .unwrap_or_else(|_| {
                                set_runtime_error(
                                    caller.data(),
                                    "TypeError: Proxy apply trap failed".to_string(),
                                );
                                value::encode_undefined()
                            });
                        }
                    }
                    return reflect_apply_impl_async(&mut caller, entry.target, this_val, &args)
                        .await;
                }
                value::encode_undefined()
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "proxy.construct",
        |mut caller: Caller<'_, RuntimeState>,
         (proxy, _this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let handle = value::decode_proxy_handle(proxy) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if let Some(_exc) = check_proxy_revoked(&mut caller, &entry, "construct") {
                        set_runtime_error(
                            caller.data(),
                            "TypeError: Cannot perform construct on a proxy that has been revoked"
                                .to_string(),
                        );
                        return value::encode_undefined();
                    }
                    if !is_constructor_in_runtime(&mut caller, entry.target) {
                        set_runtime_error(
                            caller.data(),
                            "TypeError: Proxy target must be a constructor".to_string(),
                        );
                        return value::encode_undefined();
                    }
                    let args: Vec<i64> = (0..args_count.max(0))
                        .map(|i| read_shadow_arg(&mut caller, args_base, i as u32))
                        .collect();
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap =
                            read_object_property_by_name(&mut caller, handler_ptr, "construct")
                                .unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let arr = alloc_array(&mut caller, args.len() as u32);
                            for (i, &arg) in args.iter().enumerate() {
                                set_array_elem(&mut caller, arr, i as i32, arg);
                            }
                            let trap_result = call_wasm_callback_async(
                                &mut caller,
                                trap,
                                entry.handler,
                                &[entry.target, arr, proxy],
                            )
                            .await;
                            return match trap_result {
                                Ok(res) => {
                                    if !value::is_js_object(res) {
                                        set_runtime_error(
                                            caller.data(),
                                            "TypeError: Proxy construct trap returned non-object"
                                                .to_string(),
                                        );
                                        value::encode_undefined()
                                    } else {
                                        res
                                    }
                                }
                                Err(e) => {
                                    set_runtime_error(
                                        caller.data(),
                                        format!("TypeError: Proxy construct trap failed: {}", e),
                                    );
                                    value::encode_undefined()
                                }
                            };
                        }
                    }
                    return reflect_construct_impl_async(&mut caller, entry.target, &args, proxy)
                        .await;
                }
                value::encode_undefined()
            })
        },
    )?;

    Ok(())
}
