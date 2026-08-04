//! 模块后端 adapter：只注册 wasmtime imports 与构造 require.cache Proxy。
//!
//! CJS require/resolve/cache、import.meta.resolve 与 dynamic import 编排位于
//! `wjsm_builtins::modules`；动态模块实例化仍由后端专属 loader 承担。

use std::path::PathBuf;

use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};
use wjsm_host::ExecContext;

use crate::*;

pub(crate) fn define_modules(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let create_require_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, filename_val: i64| -> i64 {
            let filename = crate::exec_context_impl::WasmExecContext::new(&mut caller)
                .render_value(filename_val);
            create_native_callable(
                caller.data(),
                NativeCallable::CjsRequire {
                    referrer: RuntimeModuleReferrer::Path(PathBuf::from(filename)),
                },
            )
        },
    );
    linker.define(&mut store, "env", "cjs_create_require", create_require_fn)?;

    let register_module_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         filename_val: i64,
         module_obj: i64,
         exports_obj: i64| {
            let filename = crate::exec_context_impl::WasmExecContext::new(&mut caller)
                .render_value(filename_val);
            let key = RuntimeModuleKey::File(PathBuf::from(filename));
            let mut registry = caller
                .data()
                .module_registry
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if registry.is_loading(&key) {
                registry.begin_loading(key, None, module_obj, exports_obj);
            } else {
                registry.finish_loaded(key, None, module_obj, exports_obj, exports_obj);
            }
        },
    );
    linker.define(&mut store, "env", "cjs_register_module", register_module_fn)?;

    let import_meta_resolve_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, filename_val: i64| -> i64 {
            wjsm_builtins::modules::create_import_meta_resolve(
                &mut crate::exec_context_impl::WasmExecContext::new(&mut caller),
                filename_val,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "import_meta_resolve",
        import_meta_resolve_fn,
    )?;

    linker.func_wrap_async(
        "env",
        "dynamic_import_runtime",
        |mut caller: Caller<'_, RuntimeState>, (referrer_val, specifier_val): (i64, i64)| {
            Box::new(async move {
                wjsm_builtins::modules::call_runtime_dynamic_import(
                    &mut crate::exec_context_impl::WasmExecContext::new(&mut caller),
                    referrer_val,
                    specifier_val,
                )
                .await
            })
        },
    )?;

    Ok(())
}

pub(crate) fn create_require_cache_proxy(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let Some(env) = WasmEnv::from_caller(caller) else {
        return value::encode_undefined();
    };
    let target = alloc_host_object(caller, &env, 0);
    let root_len = caller.data().push_host_temp_roots([target]);
    let handler = alloc_host_object(caller, &env, 5);
    caller.data().truncate_host_temp_roots(root_len);
    let root_len = caller.data().push_host_temp_roots([target, handler]);
    attach_require_cache_trap(caller, handler, "get", CjsRequireCacheTrapKind::Get);
    attach_require_cache_trap(caller, handler, "has", CjsRequireCacheTrapKind::Has);
    attach_require_cache_trap(
        caller,
        handler,
        "deleteProperty",
        CjsRequireCacheTrapKind::DeleteProperty,
    );
    attach_require_cache_trap(caller, handler, "ownKeys", CjsRequireCacheTrapKind::OwnKeys);
    attach_require_cache_trap(
        caller,
        handler,
        "getOwnPropertyDescriptor",
        CjsRequireCacheTrapKind::GetOwnPropertyDescriptor,
    );
    let proxy = {
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
        value::encode_proxy_handle(handle)
    };
    caller.data().truncate_host_temp_roots(root_len);
    proxy
}

fn attach_require_cache_trap(
    caller: &mut Caller<'_, RuntimeState>,
    handler: i64,
    name: &str,
    kind: CjsRequireCacheTrapKind,
) {
    let trap = create_native_callable(caller.data(), NativeCallable::CjsRequireCacheTrap { kind });
    let _ = define_host_data_property_from_caller(caller, handler, name, trap);
}
