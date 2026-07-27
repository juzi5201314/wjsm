//! core async import 薄注册；算法位于 `wjsm-builtins::core_async`。

use anyhow::Result;
use wasmtime::{Caller, Linker, Store};

use crate::*;

pub(crate) fn define_core_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    use crate::exec_context_impl::WasmExecContext;
    linker.func_wrap_async(
        "env",
        "op_in",
        |mut caller: Caller<'_, RuntimeState>, (object, prop): (i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core_async::op_in(&mut ctx, object, prop).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "iterator_from",
        |mut caller: Caller<'_, RuntimeState>, (val,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core_async::iterator_from(&mut ctx, val).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "iterator_next",
        |mut caller: Caller<'_, RuntimeState>, (handle,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core_async::iterator_next(&mut ctx, handle).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "iterator_done",
        |mut caller: Caller<'_, RuntimeState>, (handle,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core_async::iterator_done(&mut ctx, handle).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "iterator_close",
        |mut caller: Caller<'_, RuntimeState>, (handle, completion): (i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core_async::iterator_close(&mut ctx, handle, completion).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "iterator_step_value",
        |mut caller: Caller<'_, RuntimeState>, (handle,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core_async::iterator_step_value(&mut ctx, handle).await
            })
        },
    )?;
    Ok(())
}
