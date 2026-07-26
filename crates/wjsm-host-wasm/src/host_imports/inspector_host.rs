//! Inspector 宿主 import：`env.debug_break(line, col, flags)` 薄注册。
//!
//! 暂停循环本体在 `inspector::pause::debug_break_body`，经
//! `WasmExecContext::debug_break` → `wjsm_builtins::inspector_host::debug_break` 调用。

use anyhow::Result;
use wasmtime::{Caller, Linker, Store};

use crate::*;

pub(crate) fn define_inspector_host(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "debug_break",
        |mut caller: Caller<'_, RuntimeState>, (line, col, flags): (i32, i32, i32)| {
            Box::new(async move {
                let mut ctx = crate::exec_context_impl::WasmExecContext::new(&mut caller);
                wjsm_builtins::inspector_host::debug_break(&mut ctx, line, col, flags).await
            })
        },
    )?;
    Ok(())
}
