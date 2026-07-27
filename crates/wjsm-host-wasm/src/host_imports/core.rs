//! core import 注册壳；JavaScript 语义位于 `wjsm-builtins::core`。

use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};

use crate::RuntimeState;
use crate::exec_context_impl::WasmExecContext;

pub(crate) fn define_core(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::render::write_console_values_impl(&mut ctx, args_base, args_count, None);
        },
    );
    linker.define(&mut store, "env", "console_log", f)?;
    for (name, prefix) in [
        ("console_error", "error"),
        ("console_warn", "warn"),
        ("console_info", "info"),
        ("console_debug", "debug"),
        ("console_trace", "trace"),
    ] {
        let f = Func::wrap(
            &mut store,
            move |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::render::write_console_values_impl(
                    &mut ctx,
                    args_base,
                    args_count,
                    Some(prefix),
                );
            },
        );
        linker.define(&mut store, "env", name, f)?;
    }

    let f = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| wjsm_builtins::core::f64_mod(a, b),
    );
    linker.define(&mut store, "env", "f64_mod", f)?;
    let f = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| wjsm_builtins::core::f64_pow(a, b),
    );
    linker.define(&mut store, "env", "f64_pow", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::legacy_throw(&mut ctx, val);
        },
    );
    linker.define(&mut store, "env", "throw", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, handle: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::iterator_value(&mut ctx, handle)
        },
    );
    linker.define(&mut store, "env", "iterator_value", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::enumerator_from(&mut ctx, val)
        },
    );
    linker.define(&mut store, "env", "enumerator_from", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, handle: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::enumerator_next(&mut ctx, handle)
        },
    );
    linker.define(&mut store, "env", "enumerator_next", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, handle: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::enumerator_key(&mut ctx, handle)
        },
    );
    linker.define(&mut store, "env", "enumerator_key", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, handle: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::enumerator_done(&mut ctx, handle)
        },
    );
    linker.define(&mut store, "env", "enumerator_done", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::typeof_impl(&mut ctx, val)
        },
    );
    linker.define(&mut store, "env", "typeof", f)?;
    linker.func_wrap_async(
        "env",
        "op_instanceof",
        |mut caller: Caller<'_, RuntimeState>, (object, constructor): (i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::core::op_instanceof(&mut ctx, object, constructor).await
            })
        },
    )?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::string_concat(&mut ctx, a, b)
        },
    );
    linker.define(&mut store, "env", "string_concat", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::string_concat_va(&mut ctx, args_base, args_count)
        },
    );
    linker.define(&mut store, "env", "string_concat_va", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, object: i64, key: i32, descriptor: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::define_property_impl(&mut ctx, object, key as u32, descriptor)
        },
    );
    linker.define(&mut store, "env", "define_property", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, object: i64, key: i32| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::get_own_prop_desc(&mut ctx, object, key)
        },
    );
    linker.define(&mut store, "env", "get_own_prop_desc", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::abstract_eq_impl(&mut ctx, a, b)
        },
    );
    linker.define(&mut store, "env", "abstract_eq", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::strict_eq_impl(&mut ctx, a, b)
        },
    );
    linker.define(&mut store, "env", "strict_eq", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::abstract_compare_impl(&mut ctx, a, b)
        },
    );
    linker.define(&mut store, "env", "abstract_compare", f)?;

    register_gc_imports(linker, store)
}

fn register_gc_imports(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>| {
        let Some(env) = crate::wasm_env::WasmEnv::from_caller(&mut caller) else {
            return;
        };
        if let Some(global) = env.gc_alloc_bytes {
            let _ = global.set(&mut caller, wasmtime::Val::I32(0));
        }
        let algorithm = caller.data().gc_algorithm.as_str();
        let mut stats =
            crate::runtime_gc::active_zgc::collect_dispatch(&mut caller, &env, algorithm);
        let next_trigger = {
            let heap_limit = caller.data().heap_access_v2().heap_limit_bytes();
            let mut scheduler = caller
                .data()
                .gc_scheduler
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            scheduler.after_cycle(
                stats.heap_used_bytes,
                0,
                heap_limit.min(usize::MAX as u64) as usize,
            );
            scheduler.trigger_bytes.min(i32::MAX as usize).max(1) as i32
        };
        if let Some(global) = env.gc_trigger_bytes {
            let _ = global.set(&mut caller, wasmtime::Val::I32(next_trigger));
        }
        if algorithm != "zgc" {
            stats.pause_ns_max = 0;
            stats.pause_ns_total = 0;
            stats.pause_count = 0;
        }
        caller.data().store_last_gc_stats(algorithm, stats);
    });
    linker.define(&mut store, "env", "gc_safepoint_poll", f)?;

    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>| {
        let Some(env) = crate::wasm_env::WasmEnv::from_caller(&mut caller) else {
            return;
        };
        let (_, _, barrier_event_buf_base) = caller.data().heap_layout_boundaries();
        if barrier_event_buf_base != 0
            && let Some(global) = env.barrier_buf_ptr
        {
            let _ = global.set(
                &mut caller,
                wasmtime::Val::I32(barrier_event_buf_base as i32),
            );
        }
    });
    linker.define(&mut store, "env", "gc_barrier_flush", f)?;

    let f = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, handle: i32| -> i32 {
            if handle < 0 {
                return 0;
            }
            caller
                .data()
                .heap_access_v2()
                .resolve_handle(handle as u32)
                .ok()
                .and_then(|address| i32::try_from(address).ok())
                .unwrap_or(0)
        },
    );
    linker.define(&mut store, "env", "gc_load_barrier_slow", f)?;

    let f = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>| -> i32 {
        caller
            .data()
            .handle_free_list
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pop()
            .map(|handle| handle as i32)
            .unwrap_or(-1)
    });
    linker.define(&mut store, "env", "gc_take_freed_handle", f)?;
    Ok(())
}
