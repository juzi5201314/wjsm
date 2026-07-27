//! Reflect + Proxy async 注册层（薄包装）。
//!
//! 算法在 `wjsm_builtins::proxy_reflect_async`；闭包体仅 `WasmExecContext::new`
//! + 一行调用。

use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Linker};


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
                wjsm_builtins::proxy_reflect_async::reflect_has_async(&mut ctx, target, prop).await
            })
        },
    )?;

    // ── Reflect.deleteProperty ──
    linker.func_wrap_async(
        "env",
        "reflect_delete_property",
        |mut caller: Caller<'_, RuntimeState>, (target, prop): (i64, i64)| {
            Box::new(async move {
                wjsm_builtins::proxy_entrypoints::reflect_delete_property(
                    &mut WasmExecContext::new(&mut caller),
                    target,
                    prop,
                )
                .await
            })
        },
    )?;

    // ── Reflect.apply ──
    linker.func_wrap_async(
        "env",
        "reflect_apply",
        |mut caller: Caller<'_, RuntimeState>, (target, this_arg, args_array): (i64, i64, i64)| {
            Box::new(async move {
                wjsm_builtins::proxy_entrypoints::reflect_apply(
                    &mut WasmExecContext::new(&mut caller),
                    target,
                    this_arg,
                    args_array,
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
                wjsm_builtins::proxy_entrypoints::reflect_construct(
                    &mut WasmExecContext::new(&mut caller),
                    target,
                    args_array,
                    new_target,
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
                wjsm_builtins::proxy_entrypoints::reflect_get_prototype_of(
                    &mut WasmExecContext::new(&mut caller),
                    target,
                )
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
                wjsm_builtins::proxy_entrypoints::reflect_set_prototype_of(
                    &mut WasmExecContext::new(&mut caller),
                    target,
                    proto,
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
                wjsm_builtins::proxy_entrypoints::reflect_is_extensible(
                    &mut WasmExecContext::new(&mut caller),
                    target,
                )
                .await
            })
        },
    )?;

    // ── Reflect.preventExtensions ──
    linker.func_wrap_async(
        "env",
        "reflect_prevent_extensions",
        |mut caller: Caller<'_, RuntimeState>, (target,): (i64,)| {
            Box::new(async move {
                wjsm_builtins::proxy_entrypoints::reflect_prevent_extensions(
                    &mut WasmExecContext::new(&mut caller),
                    target,
                )
                .await
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
                wjsm_builtins::proxy_entrypoints::reflect_define_property(
                    &mut WasmExecContext::new(&mut caller),
                    target,
                    prop,
                    descriptor,
                )
                .await
            })
        },
    )?;

    // ── Reflect.ownKeys ──
    linker.func_wrap_async(
        "env",
        "reflect_own_keys",
        |mut caller: Caller<'_, RuntimeState>, (target,): (i64,)| {
            Box::new(async move {
                wjsm_builtins::proxy_entrypoints::reflect_own_keys(
                    &mut WasmExecContext::new(&mut caller),
                    target,
                )
                .await
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
                wjsm_builtins::proxy_entrypoints::proxy_apply(
                    &mut WasmExecContext::new(&mut caller),
                    proxy,
                    this_val,
                    args_base,
                    args_count,
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
                wjsm_builtins::proxy_entrypoints::proxy_construct(
                    &mut WasmExecContext::new(&mut caller),
                    proxy,
                    args_base,
                    args_count,
                )
                .await
            })
        },
    )?;

    Ok(())
}
