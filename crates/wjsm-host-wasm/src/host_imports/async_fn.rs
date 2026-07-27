use crate::exec_context_impl::WasmExecContext;
use crate::*;
use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

pub(crate) fn define_async_fn(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let async_function_start_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, fn_table_idx: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::async_fn::async_function_start(&mut ctx, fn_table_idx)
        },
    );
    linker.define(
        &mut store,
        "env",
        "async_function_start",
        async_function_start_fn,
    )?;

    linker.func_wrap_async(
        "env",
        "async_function_resume",
        |mut caller: Caller<'_, RuntimeState>,
         (fn_table_idx, continuation, state, resume_val, completion_raw): (
            i64,
            i64,
            i64,
            i64,
            i64,
        )| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::async_fn::async_function_resume(
                    &mut ctx,
                    fn_table_idx,
                    continuation,
                    state,
                    resume_val,
                    completion_raw,
                )
                .await
            })
        },
    )?;

    let async_function_suspend_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         continuation: i64,
         awaited_promise: i64,
         state: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::async_fn::async_function_suspend(
                &mut ctx,
                continuation,
                awaited_promise,
                state,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "async_function_suspend",
        async_function_suspend_fn,
    )?;

    let continuation_create_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         fn_table_idx: i64,
         outer_promise: i64,
         captured_var_count: i64|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::async_fn::continuation_create(
                &mut ctx,
                fn_table_idx,
                outer_promise,
                captured_var_count,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "continuation_create",
        continuation_create_fn,
    )?;

    let continuation_save_var_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, continuation: i64, slot: i64, val: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::async_fn::continuation_save_var(&mut ctx, continuation, slot, val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "continuation_save_var",
        continuation_save_var_fn,
    )?;

    let continuation_load_var_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, continuation: i64, slot: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::async_fn::continuation_load_var(&mut ctx, continuation, slot)
        },
    );
    linker.define(
        &mut store,
        "env",
        "continuation_load_var",
        continuation_load_var_fn,
    )?;
    Ok(())
}
