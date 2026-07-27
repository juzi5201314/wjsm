//! Promise host imports — 薄注册层。
//!
//! 算法逻辑在 `wjsm-builtins::promise`，本文件仅做 `WasmExecContext::new(caller)`
//! + 委托调用 + wasmtime `Func::wrap` 注册。

use crate::RuntimeState;
use crate::exec_context_impl::WasmExecContext;
use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};
use wjsm_host::ExecContext;

pub(crate) fn define_promise(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    // ── promise_create ──
    let promise_create_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            ctx.alloc_promise()
        },
    );
    linker.define(&mut store, "env", "promise_create", promise_create_fn)?;

    // ── promise_instance_resolve ──
    let promise_instance_resolve_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, promise: i64, value: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            ctx.resolve_promise(promise, value);
        },
    );
    linker.define(
        &mut store,
        "env",
        "promise_instance_resolve",
        promise_instance_resolve_fn,
    )?;

    // ── promise_instance_reject ──
    let promise_instance_reject_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, promise: i64, reason: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            ctx.settle_promise(promise, wjsm_host::PromiseSettlement::Reject(reason));
        },
    );
    linker.define(
        &mut store,
        "env",
        "promise_instance_reject",
        promise_instance_reject_fn,
    )?;

    // ── promise_create_resolve_function ──
    let promise_create_resolve_function_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, promise: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            ctx.create_promise_resolving_function(promise, wjsm_host::PromiseResolvingKind::Fulfill)
        },
    );
    linker.define(
        &mut store,
        "env",
        "promise_create_resolve_function",
        promise_create_resolve_function_fn,
    )?;

    // ── promise_create_reject_function ──
    let promise_create_reject_function_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, promise: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            ctx.create_promise_resolving_function(promise, wjsm_host::PromiseResolvingKind::Reject)
        },
    );
    linker.define(
        &mut store,
        "env",
        "promise_create_reject_function",
        promise_create_reject_function_fn,
    )?;

    // ── promise_then ──
    let promise_then_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         promise: i64,
         on_fulfilled: i64,
         on_rejected: i64|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::promise::promise_then_impl(&mut ctx, promise, on_fulfilled, on_rejected)
        },
    );
    linker.define(&mut store, "env", "promise_then", promise_then_fn)?;

    // ── promise_catch ──
    let promise_catch_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, promise: i64, on_rejected: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::promise::promise_catch_impl(&mut ctx, promise, on_rejected)
        },
    );
    linker.define(&mut store, "env", "promise_catch", promise_catch_fn)?;

    // ── promise_finally ──
    let promise_finally_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, promise: i64, on_finally: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::promise::promise_finally_impl(&mut ctx, promise, on_finally)
        },
    );
    linker.define(&mut store, "env", "promise_finally", promise_finally_fn)?;

    // ── promise_resolve_static ──
    let promise_resolve_static_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64, val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::promise::promise_resolve_static_impl(&mut ctx, constructor, val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "promise_resolve_static",
        promise_resolve_static_fn,
    )?;

    // ── promise_reject_static ──
    let promise_reject_static_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64, reason: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::promise::promise_reject_static_impl(&mut ctx, constructor, reason)
        },
    );
    linker.define(
        &mut store,
        "env",
        "promise_reject_static",
        promise_reject_static_fn,
    )?;

    // ── promise_with_resolvers ──
    let promise_with_resolvers_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::promise::promise_with_resolvers_impl(&mut ctx, constructor)
        },
    );
    linker.define(
        &mut store,
        "env",
        "promise_with_resolvers",
        promise_with_resolvers_fn,
    )?;

    // ── is_promise ──
    let is_promise_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_ir::value::encode_bool(ctx.is_promise_value(val))
        },
    );
    linker.define(&mut store, "env", "is_promise", is_promise_fn)?;

    Ok(())
}
