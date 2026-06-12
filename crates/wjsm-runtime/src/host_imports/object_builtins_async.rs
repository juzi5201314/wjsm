//! Async overrides for `define_object_builtins` reentrant host imports.

use anyhow::Result;
use wasmtime::{Caller, Linker};

use super::proxy_reflect::{
    object_entries_async, object_enumerable_own_keys_async, object_get_own_property_names_async,
};
use crate::*;

pub(crate) fn define_object_builtins_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "obj_get_proto_of",
        |mut caller: Caller<'_, RuntimeState>, (obj,): (i64,)| {
            Box::new(async move {
                if !value::is_js_object(obj) {
                    return value::encode_null();
                }
                proxy_or_target_get_prototype_of_impl_async(&mut caller, obj).await
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "object.is_extensible",
        |mut caller: Caller<'_, RuntimeState>, (obj,): (i64,)| {
            Box::new(async move {
                if !value::is_js_object(obj) {
                    return value::encode_bool(false);
                }
                value::encode_bool(proxy_or_target_is_extensible_impl_async(&mut caller, obj).await)
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "object.prevent_extensions",
        |mut caller: Caller<'_, RuntimeState>, (obj,): (i64,)| {
            Box::new(async move {
                if !value::is_js_object(obj) {
                    set_runtime_error(
                        caller.data(),
                        "TypeError: Object.preventExtensions called on non-object".to_string(),
                    );
                    return obj;
                }
                let result = proxy_or_target_prevent_extensions_impl_async(&mut caller, obj).await;
                let has_error = caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex")
                    .is_some();
                if !result && value::is_proxy(obj) && !has_error {
                    set_runtime_error(
                        caller.data(),
                        "TypeError: Object.preventExtensions proxy trap returned falsy".to_string(),
                    );
                }
                obj
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "obj_keys",
        |mut caller: Caller<'_, RuntimeState>, (obj,): (i64,)| {
            Box::new(async move { object_enumerable_own_keys_async(&mut caller, obj).await })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "obj_entries",
        |mut caller: Caller<'_, RuntimeState>, (obj,): (i64,)| {
            Box::new(async move { object_entries_async(&mut caller, obj).await })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "obj_get_own_prop_names",
        |mut caller: Caller<'_, RuntimeState>, (obj,): (i64,)| {
            Box::new(async move { object_get_own_property_names_async(&mut caller, obj).await })
        },
    )?;

    Ok(())
}
