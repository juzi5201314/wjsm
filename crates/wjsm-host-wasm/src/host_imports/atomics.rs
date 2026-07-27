//! Atomics / SharedArrayBuffer import 注册壳；语义位于 `wjsm-builtins::atomics`。

use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};
use wjsm_host::AtomicsRmwOp;

use crate::RuntimeState;
use crate::exec_context_impl::WasmExecContext;

pub(crate) fn define_atomics(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, typed_array: i64, index: i64, _unused: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::atomics::load(&mut ctx, typed_array, index)
        },
    );
    linker.define(&mut store, "env", "atomics_load", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, typed_array: i64, index: i64, value: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::atomics::store(&mut ctx, typed_array, index, value)
        },
    );
    linker.define(&mut store, "env", "atomics_store", f)?;

    for (name, op) in [
        ("atomics_add", AtomicsRmwOp::Add),
        ("atomics_sub", AtomicsRmwOp::Sub),
        ("atomics_and", AtomicsRmwOp::And),
        ("atomics_or", AtomicsRmwOp::Or),
        ("atomics_xor", AtomicsRmwOp::Xor),
        ("atomics_exchange", AtomicsRmwOp::Exchange),
    ] {
        let f = Func::wrap(
            &mut store,
            move |mut caller: Caller<'_, RuntimeState>,
                  typed_array: i64,
                  index: i64,
                  value: i64| {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::atomics::rmw(&mut ctx, typed_array, index, value, op)
            },
        );
        linker.define(&mut store, "env", name, f)?;
    }

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         typed_array: i64,
         index: i64,
         expected: i64,
         replacement: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::atomics::compare_exchange(
                &mut ctx,
                typed_array,
                index,
                expected,
                replacement,
            )
        },
    );
    linker.define(&mut store, "env", "atomics_compare_exchange", f)?;

    let f = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, size: i64| wjsm_builtins::atomics::is_lock_free(size),
    );
    linker.define(&mut store, "env", "atomics_is_lock_free", f)?;
    let f = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>| {
        wjsm_builtins::atomics::pause()
    });
    linker.define(&mut store, "env", "atomics_pause", f)?;

    linker.func_wrap_async(
        "env",
        "atomics_wait",
        |mut caller: Caller<'_, RuntimeState>,
         (typed_array, index, expected, timeout): (i64, i64, i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::atomics::wait(&mut ctx, typed_array, index, expected, timeout).await
            })
        },
    )?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, typed_array: i64, index: i64, count: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::atomics::notify(&mut ctx, typed_array, index, count)
        },
    );
    linker.define(&mut store, "env", "atomics_notify", f)?;
    linker.func_wrap_async(
        "env",
        "atomics_wait_async",
        |mut caller: Caller<'_, RuntimeState>,
         (typed_array, index, expected, timeout): (i64, i64, i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::atomics::wait_async(&mut ctx, typed_array, index, expected, timeout)
                    .await
            })
        },
    )?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, length: i64, options: i64, target: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::atomics::shared_arraybuffer_constructor(
                &mut ctx, length, options, target,
            )
        },
    );
    linker.define(&mut store, "env", "sharedarraybuffer_constructor", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::atomics::shared_arraybuffer_byte_length(&mut ctx, this_val)
        },
    );
    linker.define(&mut store, "env", "sharedarraybuffer_proto_byte_length", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, begin: i64, end: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::atomics::shared_arraybuffer_slice(&mut ctx, this_val, begin, end)
        },
    );
    linker.define(&mut store, "env", "sharedarraybuffer_proto_slice", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, new_length: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::atomics::shared_arraybuffer_grow(&mut ctx, this_val, new_length)
        },
    );
    linker.define(&mut store, "env", "sharedarraybuffer_proto_grow", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::atomics::shared_arraybuffer_growable(&mut ctx, this_val)
        },
    );
    linker.define(&mut store, "env", "sharedarraybuffer_proto_growable", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::atomics::shared_arraybuffer_max_byte_length(&mut ctx, this_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "sharedarraybuffer_proto_max_byte_length",
        f,
    )?;
    let f = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, this_val: i64| {
            wjsm_builtins::atomics::shared_arraybuffer_species(this_val)
        },
    );
    linker.define(&mut store, "env", "sharedarraybuffer_proto_species", f)?;
    Ok(())
}
