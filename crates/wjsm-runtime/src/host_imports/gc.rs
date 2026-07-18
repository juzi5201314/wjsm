use anyhow::Result;
use wasmtime::{Caller, Extern, Linker, Val};
use wjsm_ir::value;

use crate::RuntimeState;

#[cfg(feature = "managed-heap-v2")]
pub(crate) fn define_v2(linker: &mut Linker<RuntimeState>) -> Result<()> {
    linker.func_wrap(
        "env",
        "gc_alloc_slow_v2",
        |mut caller: Caller<'_, RuntimeState>,
         bytes: i64,
         _heap_type: i32,
         _capacity: i32|
         -> wasmtime::Result<i64> {
            let bytes = u64::try_from(bytes).map_err(host_error)?;
            let access = caller.data().heap_access_v2().clone();
            let (start, end) = access.reserve_nlab(bytes)?;
            set_i64_global(
                &mut caller,
                wjsm_ir::HEAP_ALLOC_PTR_GLOBAL_NAME,
                start + bytes,
            )?;
            set_i64_global(&mut caller, wjsm_ir::HEAP_ALLOC_END_GLOBAL_NAME, end)?;
            Ok(start as i64)
        },
    )?;
    linker.func_wrap(
        "env",
        "gc_obj_get_v2",
        |mut caller: Caller<'_, RuntimeState>, object: i64, key: i32| -> wasmtime::Result<i64> {
            let handle = value::decode_handle(object);
            let raw_key = key as u32;
            if value::is_proxy(object) {
                return Ok(crate::host_imports::get_method::get_by_name_id_sync(
                    &mut caller,
                    object,
                    raw_key,
                ));
            }
            let key = property_key(&mut caller, key)?;
            let access = caller.data().heap_access_v2().clone();
            let property = access.get_property_slot_on_proto_chain(handle, key)?;
            read_v2_property(&mut caller, object, property)
        },
    )?;
    linker.func_wrap(
        "env",
        "gc_obj_set_v2",
        |mut caller: Caller<'_, RuntimeState>,
         object: i64,
         key: i32,
         new_value: i64|
         -> wasmtime::Result<()> {
            let raw_key = key as u32;
            let key = property_key(&mut caller, key)?;
            if value::is_proxy(object) {
                return set_proxy_property_v2(&mut caller, object, raw_key, key, new_value);
            }
            let handle = value::decode_handle(object);
            caller
                .data()
                .heap_access_v2()
                .set_property(handle, key, new_value as u64)?;
            Ok(())
        },
    )?;
    linker.func_wrap(
        "env",
        "gc_obj_delete_v2",
        |mut caller: Caller<'_, RuntimeState>, object: i64, key: i32| -> wasmtime::Result<i64> {
            let handle = value::decode_handle(object);
            let key = property_key(&mut caller, key)?;
            let deleted = caller
                .data()
                .heap_access_v2()
                .delete_property(handle, key)?;
            Ok(value::encode_bool(deleted))
        },
    )?;
    linker.func_wrap(
        "env",
        "gc_arr_new_v2",
        |mut caller: Caller<'_, RuntimeState>, capacity: i32| -> wasmtime::Result<i32> {
            let capacity = u32::try_from(capacity).map_err(host_error)?;
            let prototype = ensure_v2_array_prototype(&mut caller)?;
            let handle = take_next_handle(&mut caller)?;
            let bytes = u64::from(capacity)
                .checked_mul(8)
                .and_then(|elements| {
                    elements.checked_add(wjsm_ir::constants::HEAP_OBJECT_HEADER_SIZE as u64)
                })
                .ok_or_else(|| wasmtime::Error::msg("V2 array size overflow"))?;
            let access = caller.data().heap_access_v2().clone();
            let (object, end) = access.reserve_nlab(bytes)?;
            access.publish_array(handle, object, prototype, capacity)?;
            set_i64_global(
                &mut caller,
                wjsm_ir::HEAP_ALLOC_PTR_GLOBAL_NAME,
                object + bytes,
            )?;
            set_i64_global(&mut caller, wjsm_ir::HEAP_ALLOC_END_GLOBAL_NAME, end)?;
            Ok(handle as i32)
        },
    )?;
    linker.func_wrap(
        "env",
        "gc_elem_get_v2",
        |caller: Caller<'_, RuntimeState>, array: i64, index: i32| -> wasmtime::Result<i64> {
            let handle = value::decode_handle(array);
            let index = u32::try_from(index).map_err(host_error)?;
            Ok(caller
                .data()
                .heap_access_v2()
                .get_element(handle, index)?
                .unwrap_or(value::encode_undefined() as u64) as i64)
        },
    )?;
    linker.func_wrap(
        "env",
        "gc_elem_set_v2",
        |caller: Caller<'_, RuntimeState>,
         array: i64,
         index: i32,
         new_value: i64|
         -> wasmtime::Result<()> {
            let handle = value::decode_handle(array);
            let index = u32::try_from(index).map_err(host_error)?;
            caller
                .data()
                .heap_access_v2()
                .set_element(handle, index, new_value as u64)?;
            Ok(())
        },
    )?;
    Ok(())
}
#[cfg(feature = "managed-heap-v2")]
fn property_key(caller: &mut Caller<'_, RuntimeState>, key: i32) -> wasmtime::Result<u32> {
    crate::property_key::canonicalize_v2_name_id(caller, key as u32).ok_or_else(|| {
        wasmtime::Error::msg(format!(
            "V2 property key offset {} is outside main memory",
            key as u32
        ))
    })
}

#[cfg(feature = "managed-heap-v2")]
fn read_v2_property(
    caller: &mut Caller<'_, RuntimeState>,
    receiver: i64,
    property: Option<crate::runtime_gc::HeapAccessV2Property>,
) -> wasmtime::Result<i64> {
    let Some(property) = property else {
        return Ok(value::encode_undefined());
    };
    if property.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 == 0 {
        return Ok(property.value as i64);
    }
    if value::is_undefined(property.getter as i64) {
        return Ok(value::encode_undefined());
    }
    let runtime = tokio::runtime::Handle::current();
    tokio::task::block_in_place(|| {
        runtime.block_on(crate::runtime_host_helpers::call_wasm_callback_async(
            caller,
            property.getter as i64,
            receiver,
            &[],
        ))
    })
    .map_err(host_error)
}

#[cfg(feature = "managed-heap-v2")]
fn ensure_v2_array_prototype(caller: &mut Caller<'_, RuntimeState>) -> wasmtime::Result<u32> {
    let current = get_i32_global(caller, "__array_proto_handle")? as u32;
    let values_key = crate::property_key::encode_runtime_string_name_id(
        crate::property_key::intern_runtime_property_key(
            caller.data(),
            crate::runtime_string::RuntimeString::from_utf8_str("values"),
        ),
    );
    if caller
        .data()
        .heap_access_v2()
        .get_property(current, values_key)
        .ok()
        .flatten()
        .is_some()
    {
        return Ok(current);
    }
    let methods =
        wjsm_backend_wasm::host_import_registry::array_proto_method_specs().collect::<Vec<_>>();
    let capacity = u32::try_from(methods.len() + 4)
        .map_err(|_| wasmtime::Error::msg("V2 Array.prototype method table is too large"))?;
    let prototype = crate::alloc_host_object_v2(caller, capacity);
    if !value::is_object(prototype) {
        return Err(wasmtime::Error::msg(
            "V2 Array.prototype allocation did not return an object",
        ));
    }
    let handle = value::decode_handle(prototype);
    set_i32_global(caller, "__array_proto_handle", handle as i32)?;
    let table_base = get_i32_global(caller, "__arr_proto_table_base")? as u32;
    for (offset, (_, spec)) in methods.into_iter().enumerate() {
        let name = wjsm_backend_wasm::host_import_registry::array_proto_property_name(spec.name)
            .ok_or_else(|| wasmtime::Error::msg("invalid Array.prototype method name"))?;
        let callable = value::encode_function_idx(table_base + offset as u32);
        if crate::define_host_data_property_from_caller(caller, prototype, &name, callable)
            .is_none()
        {
            return Err(wasmtime::Error::msg(
                "V2 Array.prototype method installation failed",
            ));
        }
    }
    let iterator_value =
        crate::create_native_callable(caller.data(), crate::NativeCallable::ArrayProtoValues);
    let keys = crate::create_native_callable(caller.data(), crate::NativeCallable::ArrayProtoKeys);
    let entries =
        crate::create_native_callable(caller.data(), crate::NativeCallable::ArrayProtoEntries);
    if crate::define_host_data_property_from_caller(caller, prototype, "values", iterator_value)
        .is_none()
        || crate::define_host_data_property_from_caller(caller, prototype, "keys", keys).is_none()
        || crate::define_host_data_property_from_caller(caller, prototype, "entries", entries)
            .is_none()
    {
        return Err(wasmtime::Error::msg(
            "V2 Array.prototype iterator method installation failed",
        ));
    }
    if crate::define_host_data_property_by_name_id_with_flags(
        caller,
        prototype,
        crate::encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR),
        iterator_value,
        wjsm_ir::constants::FLAG_CONFIGURABLE | wjsm_ir::constants::FLAG_WRITABLE,
    )
    .is_none()
    {
        return Err(wasmtime::Error::msg(
            "V2 Array.prototype iterator property installation failed",
        ));
    }
    Ok(handle)
}

#[cfg(feature = "managed-heap-v2")]
fn set_i32_global(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
    value: i32,
) -> wasmtime::Result<()> {
    let global = caller
        .get_export(name)
        .and_then(Extern::into_global)
        .ok_or_else(|| wasmtime::Error::msg(format!("missing {name} global")))?;
    global
        .set(&mut *caller, Val::I32(value))
        .map_err(host_error)
}

#[cfg(feature = "managed-heap-v2")]
fn set_proxy_property_v2(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    raw_key: u32,
    key: u32,
    new_value: i64,
) -> wasmtime::Result<()> {
    let handle = value::decode_proxy_handle(proxy) as usize;
    let entry = caller
        .data()
        .proxy_table
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(handle)
        .cloned()
        .ok_or_else(|| wasmtime::Error::msg("invalid V2 proxy handle"))?;
    if entry.revoked {
        return Err(wasmtime::Error::msg(
            "TypeError: Cannot perform 'set' on a proxy that has been revoked",
        ));
    }
    let trap = crate::runtime_heap::read_host_data_property_v2(caller, entry.handler, "set")
        .unwrap_or_else(value::encode_undefined);
    if value::is_undefined(trap) || value::is_null(trap) {
        return set_proxy_target_property_v2(caller, entry.target, raw_key, key, new_value);
    }
    let prop = crate::property_key::name_id_to_property_key_value(raw_key)
        .ok_or_else(|| wasmtime::Error::msg("invalid V2 proxy property key"))?;
    let runtime = tokio::runtime::Handle::current();
    let result = tokio::task::block_in_place(|| {
        runtime.block_on(crate::runtime_host_helpers::call_wasm_callback_async(
            caller,
            trap,
            entry.handler,
            &[entry.target, prop, new_value, proxy],
        ))
    })
    .map_err(host_error)?;
    if value::is_falsy(result) {
        return Err(wasmtime::Error::msg(
            "TypeError: 'set' on proxy: trap returned falsish",
        ));
    }
    Ok(())
}

#[cfg(feature = "managed-heap-v2")]
fn set_proxy_target_property_v2(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    raw_key: u32,
    key: u32,
    new_value: i64,
) -> wasmtime::Result<()> {
    if value::is_proxy(target) {
        return set_proxy_property_v2(caller, target, raw_key, key, new_value);
    }
    if !value::is_object(target) {
        return Err(wasmtime::Error::msg(
            "TypeError: Proxy target is not an object",
        ));
    }
    caller
        .data()
        .heap_access_v2()
        .set_property(value::decode_handle(target), key, new_value as u64)
        .map_err(host_error)
}

#[cfg(feature = "managed-heap-v2")]
fn take_next_handle(caller: &mut Caller<'_, RuntimeState>) -> wasmtime::Result<u32> {
    let global = caller
        .get_export("__obj_table_count")
        .and_then(Extern::into_global)
        .ok_or_else(|| wasmtime::Error::msg("missing __obj_table_count global"))?;
    let Val::I32(current) = global.get(&mut *caller) else {
        return Err(wasmtime::Error::msg("__obj_table_count is not i32"));
    };
    let next = current
        .checked_add(1)
        .ok_or_else(|| wasmtime::Error::msg("V2 handle table exhausted"))?;
    global.set(&mut *caller, Val::I32(next))?;
    Ok(current as u32)
}

#[cfg(feature = "managed-heap-v2")]
fn get_i32_global(caller: &mut Caller<'_, RuntimeState>, name: &str) -> wasmtime::Result<i32> {
    let global = caller
        .get_export(name)
        .and_then(Extern::into_global)
        .ok_or_else(|| wasmtime::Error::msg(format!("missing {name} global")))?;
    let Val::I32(value) = global.get(caller) else {
        return Err(wasmtime::Error::msg(format!("{name} is not i32")));
    };
    Ok(value)
}

#[cfg(feature = "managed-heap-v2")]
fn set_i64_global(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
    value: u64,
) -> wasmtime::Result<()> {
    let global = caller
        .get_export(name)
        .and_then(Extern::into_global)
        .ok_or_else(|| wasmtime::Error::msg(format!("missing {name} global")))?;
    global.set(caller, Val::I64(value as i64))?;
    Ok(())
}

#[cfg(feature = "managed-heap-v2")]
fn host_error(error: impl std::fmt::Display) -> wasmtime::Error {
    wasmtime::Error::msg(error.to_string())
}
