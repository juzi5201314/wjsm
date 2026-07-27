//! Promise combinator host imports — 薄注册层。
//!
//! 算法逻辑在 `wjsm-builtins::promise_combinators`，本文件仅做
//! `WasmExecContext::new(caller)` + 委托调用 + wasmtime `Func::wrap` 注册。

use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};

use crate::RuntimeState;
use crate::exec_context_impl::WasmExecContext;

pub(crate) fn define_promise_combinators(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    // ── Import 122: promise_all(i64, i64) -> i64 ─────────────────────────────────
    let promise_all_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64, arr: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::promise_combinators::promise_all_impl(&mut ctx, constructor, arr)
        },
    );
    linker.define(&mut store, "env", "promise_all", promise_all_fn)?;

    // ── Import 123: promise_race(i64, i64) -> i64 ────────────────────────────────
    let promise_race_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64, arr: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::promise_combinators::promise_race_impl(&mut ctx, constructor, arr)
        },
    );
    linker.define(&mut store, "env", "promise_race", promise_race_fn)?;

    // ── Import 124: promise_all_settled(i64, i64) -> i64 ─────────────────────────
    let promise_all_settled_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64, arr: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::promise_combinators::promise_all_settled_impl(&mut ctx, constructor, arr)
        },
    );
    linker.define(
        &mut store,
        "env",
        "promise_all_settled",
        promise_all_settled_fn,
    )?;

    // ── Import 125: promise_any(i64, i64) -> i64 ─────────────────────────────────
    let promise_any_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64, arr: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::promise_combinators::promise_any_impl(&mut ctx, constructor, arr)
        },
    );
    linker.define(&mut store, "env", "promise_any", promise_any_fn)?;

    Ok(())
}
