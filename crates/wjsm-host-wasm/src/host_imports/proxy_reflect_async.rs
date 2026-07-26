//! Reflect + Proxy async 注册层（薄包装）。
//!
//! 算法在 `wjsm_builtins::proxy_reflect_async`；闭包体仅 `WasmExecContext::new`
//! + 一行调用。

use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Linker};

use wjsm_host::{ExecContext, HeapContext, Value};

use crate::exec_context_impl::WasmExecContext;
use crate::*;

pub(crate) fn define_proxy_reflect_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    // ── Reflect.get ──
    linker.func_wrap_async(
        "env",
        "reflect_get",
        |mut caller: Caller<'_, RuntimeState>, (target, prop, receiver): (i64, i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::proxy_reflect_async::reflect_get_impl_with_receiver_async(
                    &mut ctx, target, prop, receiver,
                )
                .await
            })
        },
    )?;

    // ── Reflect.set ──
    linker.func_wrap_async(
        "env",
        "reflect_set",
        |mut caller: Caller<'_, RuntimeState>,
         (target, prop, val, receiver): (i64, i64, i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::proxy_reflect_async::reflect_set_impl_with_receiver(
                    &mut ctx, target, prop, val, receiver,
                )
                .await
            })
        },
    )?;

    // ── Reflect.has ──
    linker.func_wrap_async(
        "env",
        "reflect_has",
        |mut caller: Caller<'_, RuntimeState>, (target, prop): (i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::proxy_reflect_async::reflect_has_async(&mut ctx, target, prop)
                    .await
            })
        },
    )?;

    // ── Reflect.deleteProperty ──
    linker.func_wrap_async(
        "env",
        "reflect_delete_property",
        |mut caller: Caller<'_, RuntimeState>, (target, prop): (i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                if value::is_proxy(target) {
                    if wjsm_builtins::proxy_traps::proxy_is_revoked(&mut ctx, target) {
                        return ctx.make_type_error(
                            "TypeError: Cannot perform 'deleteProperty' on a proxy that has been revoked",
                        );
                    }
                    // Proxy deleteProperty trap
                    let (t, handler) = match wjsm_builtins::proxy_traps::proxy_trap_proxy_entry(&mut ctx, target, "deleteProperty") {
                        Ok(pair) => pair,
                        Err(exc) => return exc,
                    };
                    if let Some(trap) = wjsm_builtins::proxy_traps::proxy_trap_handler_trap(&mut ctx, handler, "deleteProperty") {
                        let prop_key = wjsm_builtins::proxy_traps::proxy_trap_property_key_value(&mut ctx, 0);
                        let _ = prop_key;
                        let result = match ctx.call_js_async(trap, handler, &[t, prop]).await {
                            Ok(v) => v,
                            Err(_) => return value::encode_bool(false),
                        };
                        return value::encode_bool(value::is_truthy(result));
                    }
                    return value::encode_bool(true);
                }
                wjsm_builtins::proxy_reflect::reflect_delete_property_impl(&mut ctx, target, prop)
            })
        },
    )?;

    // ── Reflect.apply ──
    linker.func_wrap_async(
        "env",
        "reflect_apply",
        |mut caller: Caller<'_, RuntimeState>,
         (target, this_arg, args_array): (i64, i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                if !ctx.is_callable(target) {
                    return ctx.make_type_error("TypeError: Reflect.apply target must be callable");
                }
                let args = match wjsm_builtins::proxy_reflect_async::extract_array_like_elements(
                    &mut ctx, args_array,
                )
                .await
                {
                    Ok(arr) => arr,
                    Err(err) => {
                        ctx.set_last_error(err);
                        return value::encode_undefined();
                    }
                };
                wjsm_builtins::proxy_reflect_async::reflect_apply_impl_async(
                    &mut ctx, target, this_arg, &args,
                )
                .await
            })
        },
    )?;

    // ── Reflect.construct ──
    linker.func_wrap_async(
        "env",
        "reflect_construct",
        |mut caller: Caller<'_, RuntimeState>,
         (target, args_array, new_target): (i64, i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                let n_target = if value::is_undefined(new_target) {
                    target
                } else {
                    new_target
                };
                if !ctx.is_callable(target) || !ctx.is_callable(n_target) {
                    return ctx.make_type_error(
                        "TypeError: Reflect.construct target and newTarget must be constructors",
                    );
                }
                let args = match wjsm_builtins::proxy_reflect_async::extract_array_like_elements(
                    &mut ctx, args_array,
                )
                .await
                {
                    Ok(arr) => arr,
                    Err(err) => {
                        ctx.set_last_error(err);
                        return value::encode_undefined();
                    }
                };
                wjsm_builtins::proxy_reflect_async::reflect_construct_impl_async(
                    &mut ctx, target, &args, n_target,
                )
                .await
            })
        },
    )?;

    // ── Reflect.getPrototypeOf ──
    linker.func_wrap_async(
        "env",
        "reflect_get_prototype_of",
        |mut caller: Caller<'_, RuntimeState>, (target,): (i64,)| {
            Box::new(async move {
                if !value::is_object(target)
                    && !value::is_array(target)
                    && !value::is_function(target)
                    && !value::is_proxy(target)
                    && !value::is_regexp(target)
                {
                    let mut ctx = WasmExecContext::new(&mut caller);
                    ctx.set_last_error(
                        "TypeError: Reflect.getPrototypeOf called on non-object".to_string(),
                    );
                    return value::encode_undefined();
                }
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::proxy_reflect_async::reflect_get_prototype_of_async(&mut ctx, target)
                    .await
            })
        },
    )?;

    // ── Reflect.setPrototypeOf ──
    linker.func_wrap_async(
        "env",
        "reflect_set_prototype_of",
        |mut caller: Caller<'_, RuntimeState>, (target, proto): (i64, i64)| {
            Box::new(async move {
                if !value::is_object(target)
                    && !value::is_array(target)
                    && !value::is_function(target)
                    && !value::is_proxy(target)
                {
                    let mut ctx = WasmExecContext::new(&mut caller);
                    ctx.set_last_error(
                        "TypeError: Reflect.setPrototypeOf called on non-object".to_string(),
                    );
                    return value::encode_bool(false);
                }
                if !value::is_object(proto)
                    && !value::is_null(proto)
                    && !value::is_proxy(proto)
                    && !value::is_array(proto)
                    && !value::is_function(proto)
                {
                    let mut ctx = WasmExecContext::new(&mut caller);
                    ctx.set_last_error(
                        "TypeError: Reflect.setPrototypeOf prototype must be an object or null"
                            .to_string(),
                    );
                    return value::encode_bool(false);
                }
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::proxy_reflect_async::reflect_set_prototype_of_fn_impl(
                    &mut ctx, target, proto,
                )
                .await
            })
        },
    )?;

    // ── Reflect.isExtensible ──
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
                    let mut ctx = WasmExecContext::new(&mut caller);
                    ctx.set_last_error(
                        "TypeError: Reflect.isExtensible called on non-object".to_string(),
                    );
                    return value::encode_bool(false);
                }
                let mut ctx = WasmExecContext::new(&mut caller);
                if value::is_proxy(target) {
                    let (t, handler) = match wjsm_builtins::proxy_traps::proxy_trap_proxy_entry(
                        &mut ctx, target, "isExtensible",
                    ) {
                        Ok(pair) => pair,
                        Err(exc) => return exc,
                    };
                    if let Some(trap) =
                        wjsm_builtins::proxy_traps::proxy_trap_handler_trap(&mut ctx, handler, "isExtensible")
                    {
                        let result = match ctx.call_js_async(trap, handler, &[t]).await {
                            Ok(v) => v,
                            Err(_) => return value::encode_bool(false),
                        };
                        return value::encode_bool(value::is_truthy(result));
                    }
                    return value::encode_bool(ctx.is_extensible(t));
                }
                value::encode_bool(ctx.is_extensible(target))
            })
        },
    )?;

    // ── Reflect.preventExtensions ──
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
                    let mut ctx = WasmExecContext::new(&mut caller);
                    ctx.set_last_error(
                        "TypeError: Reflect.preventExtensions called on non-object".to_string(),
                    );
                    return value::encode_bool(false);
                }
                let mut ctx = WasmExecContext::new(&mut caller);
                if value::is_proxy(target) {
                    let (t, handler) = match wjsm_builtins::proxy_traps::proxy_trap_proxy_entry(
                        &mut ctx, target, "preventExtensions",
                    ) {
                        Ok(pair) => pair,
                        Err(exc) => return exc,
                    };
                    if let Some(trap) =
                        wjsm_builtins::proxy_traps::proxy_trap_handler_trap(&mut ctx, handler, "preventExtensions")
                    {
                        let result = match ctx.call_js_async(trap, handler, &[t]).await {
                            Ok(v) => v,
                            Err(_) => return value::encode_bool(false),
                        };
                        return value::encode_bool(value::is_truthy(result));
                    }
                    return value::encode_bool(ctx.prevent_extensions(t));
                }
                value::encode_bool(ctx.prevent_extensions(target))
            })
        },
    )?;

    // ── Reflect.getOwnPropertyDescriptor ──
    linker.func_wrap_async(
        "env",
        "reflect_get_own_property_descriptor",
        |mut caller: Caller<'_, RuntimeState>, (target, prop): (i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::proxy_reflect_async::reflect_get_own_property_descriptor_on_object_async(
                    &mut ctx, target, prop,
                )
                .await
            })
        },
    )?;

    // ── Reflect.defineProperty ──
    linker.func_wrap_async(
        "env",
        "reflect_define_property",
        |mut caller: Caller<'_, RuntimeState>, (target, prop, descriptor): (i64, i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                if value::is_proxy(target) {
                    let (t, handler) = match wjsm_builtins::proxy_traps::proxy_trap_proxy_entry(
                        &mut ctx, target, "defineProperty",
                    ) {
                        Ok(pair) => pair,
                        Err(exc) => return exc,
                    };
                    if let Some(trap) =
                        wjsm_builtins::proxy_traps::proxy_trap_handler_trap(&mut ctx, handler, "defineProperty")
                    {
                        let result = match ctx.call_js_async(trap, handler, &[t, prop, descriptor]).await {
                            Ok(v) => v,
                            Err(_) => return value::encode_bool(false),
                        };
                        return value::encode_bool(value::is_truthy(result));
                    }
                    return value::encode_bool(ctx.define_property_or_throw(t, prop, descriptor));
                }
                value::encode_bool(ctx.define_property_or_throw(target, prop, descriptor))
            })
        },
    )?;

    // ── Reflect.ownKeys ──
    linker.func_wrap_async(
        "env",
        "reflect_own_keys",
        |mut caller: Caller<'_, RuntimeState>, (target,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                if value::is_proxy(target) {
                    return wjsm_builtins::proxy_reflect_async::proxy_own_keys_trap_async(&mut ctx, target)
                        .await;
                }
                wjsm_builtins::proxy_reflect::reflect_own_keys_impl(&mut ctx, target)
            })
        },
    )?;

    // ── Proxy.apply ──
    linker.func_wrap_async(
        "env",
        "proxy.apply",
        |mut caller: Caller<'_, RuntimeState>,
         (proxy, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                let Some(entry) = ctx.proxy_entry(value::decode_proxy_handle(proxy) as u32) else {
                    return value::encode_undefined();
                };
                if ctx.proxy_is_revoked(value::decode_proxy_handle(proxy) as u32) {
                    ctx.set_last_error(
                        "TypeError: Cannot perform call on a proxy that has been revoked"
                            .to_string(),
                    );
                    return value::encode_undefined();
                }
                if !ctx.is_callable(entry.target) {
                    ctx.set_last_error("TypeError: Proxy target must be callable".to_string());
                    return value::encode_undefined();
                }
                let args: Vec<Value> = (0..args_count.max(0))
                    .map(|i| ctx.read_shadow_arg(args_base, i as u32))
                    .collect();
                let trap = ctx.read_data_property(entry.handler, "apply");
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    let arr = ctx.alloc_array(args.len() as u32);
                    for (i, &arg) in args.iter().enumerate() {
                        ctx.array_write_elem(arr, i as u32, arg);
                    }
                    return match ctx.call_js_async(trap, entry.handler, &[entry.target, this_val, arr]).await {
                        Ok(v) => v,
                        Err(_) => {
                            ctx.set_last_error("TypeError: Proxy apply trap failed".to_string());
                            value::encode_undefined()
                        }
                    };
                }
                wjsm_builtins::proxy_reflect_async::reflect_apply_impl_async(
                    &mut ctx, entry.target, this_val, &args,
                )
                .await
            })
        },
    )?;

    // ── Proxy.construct ──
    linker.func_wrap_async(
        "env",
        "proxy.construct",
        |mut caller: Caller<'_, RuntimeState>,
         (proxy, _this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                let Some(entry) = ctx.proxy_entry(value::decode_proxy_handle(proxy) as u32) else {
                    return value::encode_undefined();
                };
                if ctx.proxy_is_revoked(value::decode_proxy_handle(proxy) as u32) {
                    ctx.set_last_error(
                        "TypeError: Cannot perform construct on a proxy that has been revoked"
                            .to_string(),
                    );
                    return value::encode_undefined();
                }
                if !ctx.is_callable(entry.target) {
                    ctx.set_last_error("TypeError: Proxy target must be a constructor".to_string());
                    return value::encode_undefined();
                }
                let args: Vec<Value> = (0..args_count.max(0))
                    .map(|i| ctx.read_shadow_arg(args_base, i as u32))
                    .collect();
                let trap = ctx.read_data_property(entry.handler, "construct");
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    let arr = ctx.alloc_array(args.len() as u32);
                    for (i, &arg) in args.iter().enumerate() {
                        ctx.array_write_elem(arr, i as u32, arg);
                    }
                    return match ctx.call_js_async(trap, entry.handler, &[entry.target, arr, proxy]).await {
                        Ok(res) => {
                            if !value::is_js_object(res) {
                                ctx.set_last_error(
                                    "TypeError: Proxy construct trap returned non-object".to_string(),
                                );
                                value::encode_undefined()
                            } else {
                                res
                            }
                        }
                        Err(e) => {
                            ctx.set_last_error(format!("TypeError: Proxy construct trap failed: {}", e));
                            value::encode_undefined()
                        }
                    };
                }
                wjsm_builtins::proxy_reflect_async::reflect_construct_impl_async(
                    &mut ctx, entry.target, &args, proxy,
                )
                .await
            })
        },
    )?;

    Ok(())
}
