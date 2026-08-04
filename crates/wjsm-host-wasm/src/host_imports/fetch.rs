use crate::exec_context_impl::WasmExecContext;
use crate::*;
use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};

// ── Public registration ─────────────────────────────────────────────────────

pub(crate) fn define_fetch(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    // fetch(i64, i64) → i64  [input, init]
    linker.func_wrap_async(
        "env",
        "fetch",
        |mut caller: Caller<'_, RuntimeState>, (input, init): (i64, i64)| {
            Box::new(async move {
                let mut context = WasmExecContext::new(&mut caller);
                wjsm_builtins::fetch::fetch(&mut context, input, init).await
            })
        },
    )?;

    let headers_constructor = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let args: Vec<i64> = (0..args_count.max(0))
                .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                .collect();
            let mut context = WasmExecContext::new(&mut caller);
            wjsm_builtins::fetch::headers::construct(&mut context, this_val, &args)
        },
    );
    linker.define(
        &mut store,
        "env",
        "headers_constructor",
        headers_constructor,
    )?;

    let request_constructor = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let args: Vec<i64> = (0..args_count.max(0))
                .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                .collect();
            let mut context = WasmExecContext::new(&mut caller);
            wjsm_builtins::fetch::construct_request(&mut context, this_val, &args)
        },
    );
    linker.define(
        &mut store,
        "env",
        "request_constructor",
        request_constructor,
    )?;

    let response_constructor = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let args: Vec<i64> = (0..args_count.max(0))
                .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                .collect();
            let mut context = WasmExecContext::new(&mut caller);
            wjsm_builtins::fetch::construct_response(&mut context, this_val, &args)
        },
    );
    linker.define(
        &mut store,
        "env",
        "response_constructor",
        response_constructor,
    )?;

    let abort_controller_constructor = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         this_val: i64,
         _args_base: i32,
         _args_count: i32|
         -> i64 {
            let mut context = WasmExecContext::new(&mut caller);
            wjsm_builtins::fetch::construct_abort_controller(&mut context, this_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "abort_controller_constructor",
        abort_controller_constructor,
    )?;

    linker.func_wrap_async(
        "env",
        "readable_stream_constructor",
        |mut caller: Caller<'_, RuntimeState>,
         (_env, _this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let args: Vec<i64> = (0..args_count.max(0))
                    .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                    .collect();
                let mut context = WasmExecContext::new(&mut caller);
                wjsm_builtins::streams::readable::construct(&mut context, &args).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "writable_stream_constructor",
        |mut caller: Caller<'_, RuntimeState>,
         (_env, _this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let args: Vec<i64> = (0..args_count.max(0))
                    .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                    .collect();
                let mut context = WasmExecContext::new(&mut caller);
                wjsm_builtins::streams::writable::construct(&mut context, &args).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "transform_stream_constructor",
        |mut caller: Caller<'_, RuntimeState>,
         (_env, _this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let args: Vec<i64> = (0..args_count.max(0))
                    .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                    .collect();
                let mut context = WasmExecContext::new(&mut caller);
                wjsm_builtins::streams::transform::construct(&mut context, &args).await
            })
        },
    )?;

    let count_queuing_strategy_constructor = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let args: Vec<i64> = (0..args_count.max(0))
                .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                .collect();
            construct_count_queuing_strategy(&mut caller, this_val, &args)
                .unwrap_or_else(value::encode_undefined)
        },
    );
    linker.define(
        &mut store,
        "env",
        "count_queuing_strategy_constructor",
        count_queuing_strategy_constructor,
    )?;

    let byte_length_queuing_strategy_constructor = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let args: Vec<i64> = (0..args_count.max(0))
                .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                .collect();
            construct_byte_length_queuing_strategy(&mut caller, this_val, &args)
                .unwrap_or_else(value::encode_undefined)
        },
    );
    linker.define(
        &mut store,
        "env",
        "byte_length_queuing_strategy_constructor",
        byte_length_queuing_strategy_constructor,
    )?;
    Ok(())
}
