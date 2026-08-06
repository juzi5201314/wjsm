use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};

use crate::exec_context_impl::WasmExecContext;
use crate::*;

pub(crate) fn define_object_builtins(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let object_is_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_is(&mut ctx, a, b)
        },
    );
    linker.define(&mut store, "env", "object.is", object_is_fn)?;

    let object_create_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, proto: i64, properties: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_create(&mut ctx, proto, properties)
        },
    );
    linker.define(&mut store, "env", "object.create", object_create_fn)?;

    let object_assign_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         target: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_assign(&mut ctx, target, args_base, args_count)
        },
    );
    linker.define(&mut store, "env", "object.assign", object_assign_fn)?;

    let object_values_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_values(&mut ctx, obj)
        },
    );
    linker.define(&mut store, "env", "object.values", object_values_fn)?;

    let object_get_own_property_symbols_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_get_own_property_symbols(&mut ctx, obj)
        },
    );
    linker.define(
        &mut store,
        "env",
        "object.get_own_property_symbols",
        object_get_own_property_symbols_fn,
    )?;

    let object_set_prototype_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, proto: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_set_prototype_of(&mut ctx, obj, proto)
        },
    );
    linker.define(
        &mut store,
        "env",
        "object.set_prototype_of",
        object_set_prototype_of_fn,
    )?;

    let object_get_own_property_descriptor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_get_own_property_descriptor(
                &mut ctx, target, prop,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "object.get_own_property_descriptor",
        object_get_own_property_descriptor_fn,
    )?;

    let object_has_own_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, prop: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_has_own(&mut ctx, obj, prop)
        },
    );
    linker.define(&mut store, "env", "object.has_own", object_has_own_fn)?;

    let object_freeze_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_freeze(&mut ctx, obj)
        },
    );
    linker.define(&mut store, "env", "object.freeze", object_freeze_fn)?;

    let object_seal_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_seal(&mut ctx, obj)
        },
    );
    linker.define(&mut store, "env", "object.seal", object_seal_fn)?;

    let object_is_frozen_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_is_frozen(&mut ctx, obj)
        },
    );
    linker.define(&mut store, "env", "object.is_frozen", object_is_frozen_fn)?;

    let object_is_sealed_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_is_sealed(&mut ctx, obj)
        },
    );
    linker.define(&mut store, "env", "object.is_sealed", object_is_sealed_fn)?;

    let object_define_properties_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, props: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_define_properties(&mut ctx, target, props)
        },
    );
    linker.define(
        &mut store,
        "env",
        "object.define_properties",
        object_define_properties_fn,
    )?;

    Ok(())
}

/// Convert a prototype value to its raw handle representation for storage in object header.
pub(crate) fn proto_handle_from_value(caller: &mut Caller<'_, RuntimeState>, proto: i64) -> u32 {
    if value::is_null(proto) {
        0xFFFF_FFFF
    } else if value::is_object(proto) {
        value::decode_object_handle(proto)
    } else if value::is_array(proto) {
        value::decode_array_handle(proto)
    } else if value::is_proxy(proto) {
        value::decode_proxy_handle(proto) | 0x8000_0000
    } else if value::is_function(proto) {
        let func_idx = value::decode_function_idx(proto);
        let base = caller
            .get_export("__function_props_base")
            .and_then(|e| e.into_global())
            .and_then(|g| g.get(&mut *caller).i32())
            .unwrap_or(0) as u32;
        base + func_idx
    } else if value::is_closure(proto) {
        let closure_idx = value::decode_closure_idx(proto) as usize;
        let func_idx = caller
            .data()
            .closures
            .lock()
            .ok()
            .and_then(|g| g.get(closure_idx).map(|e| e.func_idx))
            .unwrap_or(0);
        let base = caller
            .get_export("__function_props_base")
            .and_then(|e| e.into_global())
            .and_then(|g| g.get(&mut *caller).i32())
            .unwrap_or(0) as u32;
        base + func_idx
    } else {
        0xFFFF_FFFF
    }
}

/// Read a property from an object by string-key value (already encoded as runtime string).
pub(crate) fn read_property_by_string_key_raw(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    key_val: i64,
) -> i64 {
    // NativeCallable（内置构造器）：prototype 等静态属性在 side table 中，
    // V2 堆查不到，委托 native_callable_get_property_impl 统一分派。
    if value::is_native_callable(obj) {
        if let Some(name_id) =
            crate::property_key::property_key_value_to_name_id(caller, key_val, true)
        {
            return crate::runtime_linker::native_callable_get_property_impl(
                caller,
                obj,
                name_id as i32,
            );
        }
        return value::encode_undefined();
    }
    let key = get_string_value(caller, key_val);
    {
        let handle = value::decode_handle(obj);
        let access = caller.data().heap_access_v2().clone();
        if access.resolve_handle(handle).is_ok() {
            let key_id = crate::property_key::encode_runtime_string_name_id(
                crate::property_key::intern_runtime_property_key(caller.data(), key.clone()),
            );
            return match access
                .get_property_slot_on_proto_chain(handle, key_id)
                .ok()
                .flatten()
            {
                Some(property) if property.flags & constants::FLAG_IS_ACCESSOR as u32 != 0 => {
                    super::get_method::invoke_getter_sync(caller, property.getter as i64, obj)
                }
                Some(property) => property.value as i64,
                None => value::encode_undefined(),
            };
        }
    }
    // V2-only：`obj` 不在 handle 表则无属性可读，禁止 main memory 回落。
    value::encode_undefined()
}
