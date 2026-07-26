//! Reflect + Object 同步属性算法（薄包装层）。
//!
//! 算法在 `wjsm_builtins::proxy_reflect`；本文件保留 `pub(crate)` 薄包装
//! 供未迁移的 host_imports 文件（array_object.rs / core.rs）调用。
//!
//! 高层 async 算法在 `proxy_reflect_async` 中。

use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::exec_context_impl::WasmExecContext;
use crate::*;

/// `Reflect.getOwnPropertyDescriptor` 同步路径（薄包装）。
pub(crate) fn reflect_get_own_property_descriptor_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
) -> i64 {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::proxy_reflect::reflect_get_own_property_descriptor_impl(&mut ctx, target, prop)
}

/// `Reflect.ownKeys` 同步路径（薄包装）。
pub(crate) fn reflect_own_keys_impl(caller: &mut Caller<'_, RuntimeState>, target: i64) -> i64 {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::proxy_reflect::reflect_own_keys_impl(&mut ctx, target)
}


/// Proxy create / revocable 注册（保留在 host-wasm，需要 NativeCallable 表）。
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
