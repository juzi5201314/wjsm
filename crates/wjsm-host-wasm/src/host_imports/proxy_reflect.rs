//! Reflect + Object 同步属性算法（薄包装层）。
//!
//! 算法在 `wjsm_builtins::proxy_reflect`；本文件仅保留 getOwnPropertyDescriptor
//! adapter 与需要 NativeCallable 表的 Proxy create / revocable 注册。

use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::exec_context_impl::WasmExecContext;
use crate::*;


/// Proxy create / revocable 注册。
pub(crate) fn define_proxy_reflect(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let proxy_create_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, handler: i64| -> i64 {
            wjsm_builtins::proxy_entrypoints::create_proxy(
                &mut WasmExecContext::new(&mut caller),
                target,
                handler,
            )
        },
    );
    linker.define(&mut store, "env", "proxy_create", proxy_create_fn)?;

    let proxy_revocable_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, handler: i64| -> i64 {
            wjsm_builtins::proxy_entrypoints::create_revocable_proxy(
                &mut WasmExecContext::new(&mut caller),
                target,
                handler,
            )
        },
    );
    linker.define(&mut store, "env", "proxy_revocable", proxy_revocable_fn)?;

    Ok(())
}
