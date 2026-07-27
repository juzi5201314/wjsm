//! Async overrides for `define_object_builtins` reentrant host imports.
use crate::exec_context_impl::WasmExecContext;
use crate::*;
use anyhow::Result;
use wasmtime::{Caller, Linker, Store};

pub(crate) fn define_object_builtins_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    macro_rules! wrap1 {
        ($name:expr, $path:path) => {
            linker.func_wrap_async(
                "env",
                $name,
                |mut caller: Caller<'_, RuntimeState>, (obj,): (i64,)| {
                    Box::new(async move {
                        let mut ctx = WasmExecContext::new(&mut caller);
                        $path(&mut ctx, obj).await
                    })
                },
            )?;
        };
    }
    wrap1!(
        "obj_get_proto_of",
        wjsm_builtins::object_builtins_async::obj_get_proto_of
    );
    wrap1!(
        "object.is_extensible",
        wjsm_builtins::object_builtins_async::object_is_extensible
    );
    wrap1!(
        "object.prevent_extensions",
        wjsm_builtins::object_builtins_async::object_prevent_extensions
    );
    wrap1!("obj_keys", wjsm_builtins::object_builtins_async::obj_keys);
    wrap1!(
        "obj_entries",
        wjsm_builtins::object_builtins_async::obj_entries
    );
    wrap1!(
        "obj_values",
        wjsm_builtins::object_builtins_async::obj_values
    );
    wrap1!(
        "obj_get_own_prop_names",
        wjsm_builtins::object_builtins_async::obj_get_own_prop_names
    );
    wrap1!(
        "obj_get_own_prop_symbols",
        wjsm_builtins::object_builtins_async::obj_get_own_prop_symbols
    );
    linker.func_wrap_async(
        "env",
        "obj_assign",
        |mut caller: Caller<'_, RuntimeState>,
         (_env, target, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::object_builtins_async::obj_assign(
                    &mut ctx, target, args_base, args_count,
                )
                .await
            })
        },
    )?;
    Ok(())
}
