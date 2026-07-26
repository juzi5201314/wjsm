use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};
use crate::exec_context_impl::WasmExecContext;
use crate::*;

pub(crate) fn define_misc(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let is_callable_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::misc::is_callable(&mut ctx, val)
        },
    );
    linker.define(&mut store, "env", "is_callable", is_callable_fn)?;

    let is_js_object_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::misc::is_js_object(&mut ctx, val)
    });
    linker.define(&mut store, "env", "is_js_object", is_js_object_fn)?;

    let queue_microtask_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, callback: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::misc::queue_microtask(&mut ctx, callback);
        },
    );
    linker.define(&mut store, "env", "queue_microtask", queue_microtask_fn)?;

    let register_module_namespace_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, module_id: i64, namespace_obj: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::misc::register_module_namespace(&mut ctx, module_id, namespace_obj);
        },
    );
    linker.define(
        &mut store,
        "env",
        "register_module_namespace",
        register_module_namespace_fn,
    )?;

    let dynamic_import_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, module_id: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::misc::dynamic_import(&mut ctx, module_id)
        },
    );
    linker.define(&mut store, "env", "dynamic_import", dynamic_import_fn)?;

    let jsx_create_element_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, tag: i64, props: i64, children: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::misc::jsx_create_element(&mut ctx, tag, props, children)
        },
    );
    linker.define(
        &mut store,
        "env",
        "jsx_create_element",
        jsx_create_element_fn,
    )?;

    Ok(())
}
