use super::*;
use crate::exec_context_impl::WasmExecContext;

macro_rules! wrap_array_callback_async {
    ($linker:expr, $name:expr, $call:expr) => {
        $linker.func_wrap_async(
            "env",
            $name,
            |mut caller: Caller<'_, RuntimeState>,
             (_env_obj, this_val, args_base, args_count): (i64, i64, i32, i32)| {
                Box::new(async move {
                    let mut ctx = WasmExecContext::new(&mut caller);
                    $call(&mut ctx, this_val, args_base, args_count).await
                })
            },
        )?;
    };
}

pub(crate) fn define_array_object_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    wrap_array_callback_async!(
        linker,
        "arr_proto_sort",
        wjsm_builtins::reentrant::array::arr_proto_sort
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_for_each",
        wjsm_builtins::reentrant::array::arr_proto_for_each
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_map",
        wjsm_builtins::reentrant::array::arr_proto_map
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_filter",
        wjsm_builtins::reentrant::array::arr_proto_filter
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_reduce",
        wjsm_builtins::reentrant::array::arr_proto_reduce
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_reduce_right",
        wjsm_builtins::reentrant::array::arr_proto_reduce_right
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_find",
        wjsm_builtins::reentrant::array::arr_proto_find
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_find_index",
        wjsm_builtins::reentrant::array::arr_proto_find_index
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_some",
        wjsm_builtins::reentrant::array::arr_proto_some
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_every",
        wjsm_builtins::reentrant::array::arr_proto_every
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_flat_map",
        wjsm_builtins::reentrant::array::arr_proto_flat_map
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_find_last",
        wjsm_builtins::reentrant::array::arr_proto_find_last
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_find_last_index",
        wjsm_builtins::reentrant::array::arr_proto_find_last_index
    );
    wrap_array_callback_async!(
        linker,
        "arr_proto_to_sorted",
        wjsm_builtins::reentrant::array::arr_proto_to_sorted
    );

    linker.func_wrap_async(
        "env",
        "array_push_spread",
        |mut caller: Caller<'_, RuntimeState>, (arr, iterable): (i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::reentrant::array::array_push_spread(&mut ctx, arr, iterable).await
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "func_call",
        |mut caller: Caller<'_, RuntimeState>,
         (func, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::reentrant::array::func_call(
                    &mut ctx, func, this_val, args_base, args_count,
                )
                .await
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "func_apply",
        |mut caller: Caller<'_, RuntimeState>, (func, this_val, args_array): (i64, i64, i64)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::reentrant::array::func_apply(&mut ctx, func, this_val, args_array)
                    .await
            })
        },
    )?;

    Ok(())
}
