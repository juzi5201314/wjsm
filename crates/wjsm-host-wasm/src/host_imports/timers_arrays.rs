use crate::exec_context_impl::WasmExecContext;
use crate::*;
use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};

pub(crate) fn define_timers_arrays(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    macro_rules! wrap {
        ($name:expr, $arity:tt, $call:expr) => {{
            let f = Func::wrap(&mut store, $call);
            linker.define(&mut store, "env", $name, f)?;
        }};
    }

    wrap!("closure_create", 2, |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                func_ref: i64,
                                env_obj: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::closure_create(&mut ctx, func_ref, env_obj)
    });
    wrap!("closure_get_func", 1, |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                  closure_idx: i32|
     -> i32 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::closure_get_func(&mut ctx, closure_idx)
    });
    wrap!("closure_get_env", 1, |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                 closure_idx: i32|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::closure_get_env(&mut ctx, closure_idx)
    });
    wrap!("arr_push", 2, |mut caller: Caller<'_, RuntimeState>,
                          arr: i64,
                          val: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_push(&mut ctx, arr, val)
    });
    wrap!("arr_push_hole", 1, |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                               arr: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_push_hole(&mut ctx, arr)
    });
    wrap!("arr_pop", 1, |mut caller: Caller<'_, RuntimeState>,
                         arr: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_pop(&mut ctx, arr)
    });
    wrap!("arr_includes", 2, |mut caller: Caller<'_, RuntimeState>,
                              arr: i64,
                              val: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_includes(&mut ctx, arr, val)
    });
    wrap!("arr_index_of", 3, |mut caller: Caller<'_, RuntimeState>,
                              arr: i64,
                              val: i64,
                              from_val: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_index_of(&mut ctx, arr, val, from_val)
    });
    wrap!("arr_join", 2, |mut caller: Caller<'_, RuntimeState>,
                          arr: i64,
                          sep_val: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_join(&mut ctx, arr, sep_val)
    });
    wrap!("arr_concat", 2, |mut caller: Caller<'_, RuntimeState>,
                            arr1: i64,
                            arr2: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_concat(&mut ctx, arr1, arr2)
    });
    wrap!("arr_slice", 3, |mut caller: Caller<'_, RuntimeState>,
                           arr: i64,
                           start: i64,
                           end: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_slice(&mut ctx, arr, start, end)
    });
    wrap!("arr_fill", 4, |mut caller: Caller<'_, RuntimeState>,
                          arr: i64,
                          val: i64,
                          start: i64,
                          end: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_fill(&mut ctx, arr, val, start, end)
    });
    wrap!("arr_reverse", 1, |mut caller: Caller<'_, RuntimeState>,
                             arr: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_reverse(&mut ctx, arr)
    });
    wrap!("arr_flat", 2, |mut caller: Caller<'_, RuntimeState>,
                          arr: i64,
                          depth: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_flat(&mut ctx, arr, depth)
    });
    wrap!("arr_init_length", 2, |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                 arr: i64,
                                 len_val: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_init_length(&mut ctx, arr, len_val)
    });
    wrap!("array_set_length", 2, |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                  arr: i64,
                                  len_val: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::array_set_length(&mut ctx, arr, len_val)
    });
    wrap!("arr_get_length", 1, |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                arr: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::timers_arrays::arr_get_length(&mut ctx, arr)
    });
    Ok(())
}
