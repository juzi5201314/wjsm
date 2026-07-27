//! Proxy trap async 注册层 + 薄包装（供 gc.rs 等未迁移文件调用）。
//!
//! 算法在 `wjsm_builtins::proxy_reflect_async`；闭包体仅 `WasmExecContext::new`
//! + 一行调用。

use super::*;
use crate::exec_context_impl::WasmExecContext;
use wjsm_host::ExecContext;

/// `proxy_trap_get`：Proxy [[Get]] 内部方法（薄包装，供 gc.rs 调用）。
pub(crate) async fn proxy_trap_internal_get_async(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    name_id: i32,
) -> i64 {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::proxy_reflect_async::proxy_trap_internal_get_async(&mut ctx, proxy, name_id)
        .await
}

/// `proxy_trap_set`：Proxy [[Set]] 内部方法（薄包装，供 gc.rs 调用）。
pub(crate) async fn proxy_trap_internal_set_async(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    name_id: i32,
    val: i64,
) {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::proxy_reflect_async::proxy_trap_internal_set_async(
        &mut ctx, proxy, name_id, val,
    )
    .await;
}

/// `proxy_trap_delete`：Proxy [[Delete]] 内部方法（薄包装，供 gc.rs 调用）。
pub(crate) async fn proxy_trap_internal_delete_async(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    name_id: i32,
) -> i64 {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::proxy_reflect_async::proxy_trap_internal_delete_async(&mut ctx, proxy, name_id)
        .await
}

pub(crate) fn define_proxy_traps_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    // ── proxy_trap_get ──
    linker.func_wrap_async(
        "env",
        "proxy_trap_get",
        |mut caller: Caller<'_, RuntimeState>, (proxy, name_id): (i64, i32)| {
            Box::new(
                async move { proxy_trap_internal_get_async(&mut caller, proxy, name_id).await },
            )
        },
    )?;

    // ── proxy_trap_set ──
    linker.func_wrap_async(
        "env",
        "proxy_trap_set",
        |mut caller: Caller<'_, RuntimeState>, (proxy, name_id, val): (i64, i32, i64)| {
            Box::new(async move {
                proxy_trap_internal_set_async(&mut caller, proxy, name_id, val).await;
            })
        },
    )?;

    // ── proxy_trap_delete ──
    linker.func_wrap_async(
        "env",
        "proxy_trap_delete",
        |mut caller: Caller<'_, RuntimeState>, (proxy, name_id): (i64, i32)| {
            Box::new(
                async move { proxy_trap_internal_delete_async(&mut caller, proxy, name_id).await },
            )
        },
    )?;

    // ── obj_get_runtime_key ──
    linker.func_wrap_async(
        "env",
        "obj_get_runtime_key",
        |mut caller: Caller<'_, RuntimeState>, (obj, name_id): (i64, i32)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                let key = ctx
                    .name_id_to_property_key_value(name_id as u32)
                    .unwrap_or_else(value::encode_undefined);
                ctx.reflect_get_sync(obj, key, obj)
            })
        },
    )?;

    // ── obj_set_runtime_key ──
    linker.func_wrap_async(
        "env",
        "obj_set_runtime_key",
        |mut caller: Caller<'_, RuntimeState>, (obj, name_id, val): (i64, i32, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                let key = ctx
                    .name_id_to_property_key_value(name_id as u32)
                    .unwrap_or_else(value::encode_undefined);
                let Some(handle) = ctx.handle_index_of(obj) else {
                    return;
                };
                let Some(nid) = ctx.property_value_to_name_id(key, true) else {
                    return;
                };
                ctx.define_data_property_with_flags(
                    handle,
                    nid,
                    val,
                    (constants::FLAG_CONFIGURABLE
                        | constants::FLAG_ENUMERABLE
                        | constants::FLAG_WRITABLE) as u32,
                );
            })
        },
    )?;

    // ── obj_delete_runtime_key ──
    linker.func_wrap_async(
        "env",
        "obj_delete_runtime_key",
        |mut caller: Caller<'_, RuntimeState>, (obj, name_id): (i64, i32)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                let key = ctx
                    .name_id_to_property_key_value(name_id as u32)
                    .unwrap_or_else(value::encode_undefined);
                let Some(nid) = ctx.property_value_to_name_id(key, false) else {
                    return;
                };
                let _ =
                    wjsm_builtins::proxy_reflect::delete_property_by_name_id(&mut ctx, obj, nid);
            })
        },
    )?;

    Ok(())
}
