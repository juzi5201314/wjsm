use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};
use crate::exec_context_impl::WasmExecContext;
use crate::*;

pub(crate) fn define_private_fields(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let private_get_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::private_fields::private_get(&mut ctx, obj, key_name_id)
        },
    );
    linker.define(&mut store, "env", "private_get", private_get_fn)?;

    let private_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32, val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::private_fields::private_set(&mut ctx, obj, key_name_id, val)
        },
    );
    linker.define(&mut store, "env", "private_set", private_set_fn)?;

    let private_accessor_bind_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         obj: i64,
         key_name_id: i32,
         getter: i64,
         setter: i64|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::private_fields::private_accessor_bind(
                &mut ctx, obj, key_name_id, getter, setter,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "private_accessor_bind",
        private_accessor_bind_fn,
    )?;

    let private_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::private_fields::private_has(&mut ctx, obj, key_name_id)
        },
    );
    linker.define(&mut store, "env", "private_has", private_has_fn)?;

    Ok(())
}
