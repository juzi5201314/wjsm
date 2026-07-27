use crate::exec_context_impl::WasmExecContext;
use crate::*;
use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};
pub(crate) fn define_get_builtin_global(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, name_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::get_builtin_global::get_builtin_global(&mut ctx, name_val)
        },
    );
    linker.define(&mut store, "env", "get_builtin_global", f)?;
    Ok(())
}
