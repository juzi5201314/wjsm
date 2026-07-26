use super::*;
use crate::exec_context_impl::WasmExecContext;

macro_rules! wrap_typedarray_callback_async {
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

pub(crate) fn define_typedarray_new_methods_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_sort",
        wjsm_builtins::reentrant::typedarray::typedarray_proto_sort
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_for_each",
        wjsm_builtins::reentrant::typedarray::typedarray_proto_for_each
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_map",
        wjsm_builtins::reentrant::typedarray::typedarray_proto_map
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_filter",
        wjsm_builtins::reentrant::typedarray::typedarray_proto_filter
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_reduce",
        wjsm_builtins::reentrant::typedarray::typedarray_proto_reduce
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_reduce_right",
        wjsm_builtins::reentrant::typedarray::typedarray_proto_reduce_right
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_find",
        wjsm_builtins::reentrant::typedarray::typedarray_proto_find
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_find_index",
        wjsm_builtins::reentrant::typedarray::typedarray_proto_find_index
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_some",
        wjsm_builtins::reentrant::typedarray::typedarray_proto_some
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_every",
        wjsm_builtins::reentrant::typedarray::typedarray_proto_every
    );

    Ok(())
}
