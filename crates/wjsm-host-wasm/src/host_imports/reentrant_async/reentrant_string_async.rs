use super::*;
use crate::exec_context_impl::WasmExecContext;

/// 兼容路径：RegExp @@replace 等仍可能直接调用；转 builtins 算法。
pub(crate) async fn string_replace_default_async_body(
    caller: &mut Caller<'_, RuntimeState>,
    receiver: i64,
    search: i64,
    replace: i64,
) -> i64 {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::reentrant::string::string_replace_default(&mut ctx, receiver, search, replace)
        .await
}

pub(crate) fn define_primitive_core_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "string_replace",
        |mut caller: Caller<'_, RuntimeState>, (receiver, search, replace): (i64, i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::reentrant::string::string_replace(
                    &mut ctx, receiver, search, replace,
                )
                .await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "string_match",
        |mut caller: Caller<'_, RuntimeState>, (receiver, regexp): (i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::reentrant::string::string_match(&mut ctx, receiver, regexp).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "string_search",
        |mut caller: Caller<'_, RuntimeState>, (receiver, regexp): (i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::reentrant::string::string_search(&mut ctx, receiver, regexp).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "string_split",
        |mut caller: Caller<'_, RuntimeState>, (receiver, sep, limit): (i64, i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::reentrant::string::string_split(&mut ctx, receiver, sep, limit).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "string_match_all",
        |mut caller: Caller<'_, RuntimeState>,
         (_env, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::reentrant::string::string_match_all(
                    &mut ctx, this_val, args_base, args_count,
                )
                .await
            })
        },
    )?;
    Ok(())
}
