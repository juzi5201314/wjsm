use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::exec_context_impl::WasmExecContext;
use crate::*;

pub(crate) fn define_weakref_finalization(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let weakref_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         _this: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::weakref_finalization::weakref_constructor(
                &mut ctx, args_base, args_count,
            )
        },
    );
    linker.define(&mut store, "env", "weakref_constructor", weakref_constructor_fn)?;

    let weakref_proto_deref_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::weakref_finalization::weakref_proto_deref(&mut ctx, this_val)
        },
    );
    linker.define(&mut store, "env", "weakref_proto_deref", weakref_proto_deref_fn)?;

    let finalization_registry_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         _this: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::weakref_finalization::finalization_registry_constructor(
                &mut ctx, args_base, args_count,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "finalization_registry_constructor",
        finalization_registry_constructor_fn,
    )?;

    let finalization_registry_proto_register_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::weakref_finalization::finalization_registry_proto_register(
                &mut ctx, this_val, args_base, args_count,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "finalization_registry_proto_register",
        finalization_registry_proto_register_fn,
    )?;

    let finalization_registry_proto_unregister_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, token: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::weakref_finalization::finalization_registry_proto_unregister(
                &mut ctx, this_val, token,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "finalization_registry_proto_unregister",
        finalization_registry_proto_unregister_fn,
    )?;

    Ok(())
}
