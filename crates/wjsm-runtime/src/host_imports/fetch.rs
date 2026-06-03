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
    // Import 31: fetch(i64) → i64
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, input: i64| -> i64 {
            // Create pending Promise immediately (return its handle)
            let promise = alloc_promise_from_caller(&mut caller, PromiseEntry::pending());
            let _promise_handle = value::decode_object_handle(promise) as usize;
            // Current backend ABI passes only input; constructors handle init objects.
            let (method, url, headers_handle, body_opt, _redirect) =
                parse_fetch_input(&mut caller, input, value::encode_undefined());
            // Perform the actual work synchronously (design decision)
            let settle_result = perform_fetch_and_build_response(
                &mut caller,
                method,
                url,
                headers_handle,
                body_opt,
            );

            match settle_result {
                Ok(response_val) => {
                    // Fulfill the promise with the Response object
                    settle_promise(
                        caller.data_mut(),
                        promise,
                        PromiseSettlement::Fulfill(response_val),
                    );
                }
                Err(type_error_msg) => {
                    // Reject with a TypeError (network failure or bad input)
                    let err = alloc_type_error_from_caller(&mut caller, &type_error_msg);
                    settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
                }
            }

            promise
        },
    );
    linker.define(&mut store, "env", "fetch", f)?;

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
    Ok(())
}
