use crate::exec_context_impl::WasmExecContext;
use crate::*;
use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

pub(crate) fn define_generator(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let generator_start_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, continuation: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::generator::generator_start(&mut ctx, continuation)
        },
    );
    linker.define(&mut store, "env", "generator_start", generator_start_fn)?;
    let generator_next_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::generator::generator_next(&mut ctx, generator, value)
        },
    );
    linker.define(&mut store, "env", "generator_next", generator_next_fn)?;
    let generator_return_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::generator::generator_return(&mut ctx, generator, value)
        },
    );
    linker.define(&mut store, "env", "generator_return", generator_return_fn)?;
    let generator_throw_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::generator::generator_throw(&mut ctx, generator, value)
        },
    );
    linker.define(&mut store, "env", "generator_throw", generator_throw_fn)?;
    Ok(())
}
