use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};
use crate::exec_context_impl::WasmExecContext;
use crate::*;

pub(crate) fn define_async_generator(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let start = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, continuation: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::async_generator::async_generator_start(&mut ctx, continuation)
    });
    linker.define(&mut store, "env", "async_generator_start", start)?;
    let next = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, g: i64, v: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::async_generator::async_generator_next(&mut ctx, g, v)
    });
    linker.define(&mut store, "env", "async_generator_next", next)?;
    let ret = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, g: i64, v: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::async_generator::async_generator_return(&mut ctx, g, v)
    });
    linker.define(&mut store, "env", "async_generator_return", ret)?;
    let thr = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, g: i64, v: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::async_generator::async_generator_throw(&mut ctx, g, v)
    });
    linker.define(&mut store, "env", "async_generator_throw", thr)?;
    Ok(())
}
