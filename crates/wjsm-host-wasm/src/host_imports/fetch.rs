use crate::*;
use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};

use super::fetch_core::*;
use super::fetch_http::*;

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
                let promise = alloc_promise_from_caller(&mut caller, PromiseEntry::pending());

                let (method, url, headers_handle, body_opt, redirect, signal_handle) =
                    parse_fetch_input(&mut caller, input, init);
                if url.is_empty() {
                    let err = alloc_type_error_from_caller(
                        &mut caller,
                        "Failed to parse URL from request.",
                    );
                    settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
                    return promise;
                }
                let suppress_resource_timing =
                    fetch_init_suppresses_resource_timing(&mut caller, init);
                let resource_timing = begin_fetch_resource_timing(
                    caller.data(),
                    url.clone(),
                    suppress_resource_timing,
                );
                // data: URL — 同步路径（保持现有行为）
                if url.starts_with("data:") {
                    mark_fetch_request_start(caller.data(), &resource_timing);
                    match perform_data_url_fetch(&mut caller, &url) {
                        Ok(response_val) => {
                            mark_fetch_response_start(caller.data(), &resource_timing, 200);
                            set_response_resource_timing(
                                &mut caller,
                                response_val,
                                resource_timing.clone(),
                            );
                            settle_promise(
                                caller.data_mut(),
                                promise,
                                PromiseSettlement::Fulfill(response_val),
                            );
                        }
                        Err(msg) => {
                            let err = alloc_type_error_from_caller(&mut caller, &msg);
                            settle_promise(
                                caller.data_mut(),
                                promise,
                                PromiseSettlement::Reject(err),
                            );
                        }
                    }
                    return promise;
                }
                // HTTP/HTTPS — 异步路径
                let guard = caller
                    .data()
                    .async_op_counter
                    .as_ref()
                    .map(|counter| counter.begin());
                let http_result = perform_http_fetch(
                    &mut caller,
                    method,
                    url,
                    headers_handle,
                    body_opt,
                    redirect,
                    signal_handle,
                    resource_timing,
                )
                .await;
                match http_result {
                    Ok(response_val) => {
                        settle_promise(
                            caller.data_mut(),
                            promise,
                            PromiseSettlement::Fulfill(response_val),
                        );
                    }
                    Err(msg) => {
                        let err = alloc_type_error_from_caller(&mut caller, &msg);
                        settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
                    }
                }
                drop(guard);
                promise
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
            construct_headers(&mut caller, this_val, &args).unwrap_or_else(value::encode_undefined)
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
            construct_request(&mut caller, this_val, &args).unwrap_or_else(value::encode_undefined)
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
            construct_response(&mut caller, this_val, &args).unwrap_or_else(value::encode_undefined)
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
         args_base: i32,
         args_count: i32|
         -> i64 {
            let args: Vec<i64> = (0..args_count.max(0))
                .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                .collect();
            construct_abort_controller(&mut caller, this_val, &args)
                .unwrap_or_else(value::encode_undefined)
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
         (_env, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let args: Vec<i64> = (0..args_count.max(0))
                    .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                    .collect();
                construct_readable_stream(&mut caller, this_val, &args)
                    .await
                    .unwrap_or_else(value::encode_undefined)
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "writable_stream_constructor",
        |mut caller: Caller<'_, RuntimeState>,
         (_env, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let args: Vec<i64> = (0..args_count.max(0))
                    .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                    .collect();
                construct_writable_stream(&mut caller, this_val, &args)
                    .await
                    .unwrap_or_else(value::encode_undefined)
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "transform_stream_constructor",
        |mut caller: Caller<'_, RuntimeState>,
         (_env, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let args: Vec<i64> = (0..args_count.max(0))
                    .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                    .collect();
                construct_transform_stream(&mut caller, this_val, &args)
                    .await
                    .unwrap_or_else(value::encode_undefined)
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

fn fetch_init_suppresses_resource_timing(caller: &mut Caller<'_, RuntimeState>, init: i64) -> bool {
    object_property(caller, init, "__wjsm_internal_no_resource_timing")
        .is_some_and(|raw| value::is_bool(raw) && value::decode_bool(raw))
}
