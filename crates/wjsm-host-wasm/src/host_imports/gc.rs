use anyhow::Result;
use wasmtime::{Caller, Linker, Val};
use wjsm_ir::value;

use crate::RuntimeState;

pub(crate) fn allocate_v2_array_handle(
    caller: &mut Caller<'_, RuntimeState>,
    capacity: u32,
) -> wasmtime::Result<u32> {
    let prototype = ensure_v2_array_prototype(caller)?;
    let handle = take_next_handle(caller)?;
    let bytes = u64::from(capacity)
        .checked_mul(8)
        .and_then(|elements| {
            elements.checked_add(wjsm_ir::constants::HEAP_OBJECT_HEADER_SIZE as u64)
        })
        .ok_or_else(|| wasmtime::Error::msg("V2 array size overflow"))?;
    let access = caller.data().heap_access_v2().clone();
    let (object, _) = crate::allocate_v2_object_bytes(caller, bytes)?;
    access.publish_array(handle, object, prototype, capacity)?;
    Ok(handle)
}

pub(crate) fn define_v2(linker: &mut Linker<RuntimeState>) -> Result<()> {
    linker.func_wrap(
        "env",
        "gc_alloc_slow",
        |mut caller: Caller<'_, RuntimeState>,
         bytes: i64,
         _heap_type: i32,
         _capacity: i32|
         -> wasmtime::Result<i64> {
            let bytes = u64::try_from(bytes).map_err(host_error)?;
            let (start, _) = crate::allocate_v2_object_bytes(&mut caller, bytes)?;
            Ok(start as i64)
        },
    )?;
    linker.func_wrap_async(
        "env",
        "gc_obj_get",
        |mut caller: Caller<'_, RuntimeState>, (object, key): (i64, i32)| {
            Box::new(async move {
                let name_id = key as u32;
                if value::is_js_object(object) && !value::is_proxy(object) {
                    caller.data().count_barrier_load();
                }
                Ok(wjsm_builtins::property::get_by_name_id(
                    &mut crate::exec_context_impl::WasmExecContext::new(&mut caller),
                    object,
                    name_id,
                )
                .await)
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "gc_obj_set",
        |mut caller: Caller<'_, RuntimeState>, (object, key, new_value): (i64, i32, i64)| {
            Box::new(async move {
                let name_id = key as u32;
                if value::is_js_object(object)
                    && !value::is_proxy(object)
                    && !value::is_regexp(object)
                {
                    caller.data().count_barrier_store();
                }
                wjsm_builtins::property::set_by_name_id(
                    &mut crate::exec_context_impl::WasmExecContext::new(&mut caller),
                    object,
                    name_id,
                    new_value,
                )
                .await;
                Ok(())
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "gc_obj_delete",
        |mut caller: Caller<'_, RuntimeState>, (object, key): (i64, i32)| {
            Box::new(async move {
                let name_id = key as u32;
                Ok(wjsm_builtins::property::delete_by_name_id(
                    &mut crate::exec_context_impl::WasmExecContext::new(&mut caller),
                    object,
                    name_id,
                )
                .await)
            })
        },
    )?;
    linker.func_wrap(
        "env",
        "gc_arr_new",
        |mut caller: Caller<'_, RuntimeState>, capacity: i32| -> wasmtime::Result<i32> {
            let capacity = u32::try_from(capacity).map_err(host_error)?;
            Ok(allocate_v2_array_handle(&mut caller, capacity)? as i32)
        },
    )?;
    linker.func_wrap_async(
        "env",
        "gc_elem_get",
        |mut caller: Caller<'_, RuntimeState>, (array, index): (i64, i32)| {
            Box::new(async move {
                if index >= 0
                    && let Some(element) = crate::runtime_typedarray::typedarray_element_read(
                        &mut caller,
                        array,
                        index as u32,
                    )
                {
                    return Ok(element);
                }
                let handle = value::decode_handle(array);
                let access = caller.data().heap_access_v2().clone();
                caller.data().count_barrier_load();
                if value::is_array(array)
                    && index >= 0
                    && access.object_type(handle).ok()
                        == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY))
                {
                    return Ok(access
                        .get_element(handle, index as u32)?
                        .unwrap_or(value::encode_undefined() as u64)
                        as i64);
                }
                let name_id = v2_index_property_key(&caller, index);
                Ok(wjsm_builtins::property::get_by_name_id(
                    &mut crate::exec_context_impl::WasmExecContext::new(&mut caller),
                    array,
                    name_id,
                )
                .await)
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "gc_elem_set",
        |mut caller: Caller<'_, RuntimeState>, (array, index, new_value): (i64, i32, i64)| {
            Box::new(async move {
                if crate::runtime_typedarray::typedarray_entry_from_value(&mut caller, array)
                    .is_some()
                {
                    if index >= 0 {
                        let _ = crate::runtime_typedarray::typedarray_element_write(
                            &mut caller,
                            array,
                            index as u32,
                            new_value,
                        );
                    }
                    return Ok(());
                }
                let handle = value::decode_handle(array);
                let access = caller.data().heap_access_v2().clone();
                caller.data().count_barrier_store();
                if value::is_array(array)
                    && index >= 0
                    && access.object_type(handle).ok()
                        == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY))
                {
                    crate::set_v2_array_element(
                        &mut caller,
                        handle,
                        index as u32,
                        new_value as u64,
                    )?;
                    return Ok(());
                }
                let name_id = v2_index_property_key(&caller, index);
                wjsm_builtins::property::set_by_name_id(
                    &mut crate::exec_context_impl::WasmExecContext::new(&mut caller),
                    array,
                    name_id,
                    new_value,
                )
                .await;
                Ok(())
            })
        },
    )?;
    Ok(())
}

/// 数值下标 → 规范化 V2 属性键（与 define_host_data_property_v2 同一 intern 表）。
fn v2_index_property_key(caller: &Caller<'_, RuntimeState>, index: i32) -> u32 {
    crate::property_key::encode_runtime_string_name_id(
        crate::property_key::intern_runtime_property_key(
            caller.data(),
            crate::runtime_string::RuntimeString::from_utf8_str(&index.to_string()),
        ),
    )
}


fn ensure_v2_array_prototype(caller: &mut Caller<'_, RuntimeState>) -> wasmtime::Result<u32> {
    let env = crate::WasmEnv::from_caller(caller)
        .ok_or_else(|| wasmtime::Error::msg("missing cached WasmEnv"))?;
    let current =
        env.array_proto_handle
            .get(&mut *caller)
            .i32()
            .ok_or_else(|| wasmtime::Error::msg("__array_proto_handle is not i32"))? as u32;
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
    env.array_proto_handle
        .set(&mut *caller, Val::I32(handle as i32))
        .map_err(host_error)?;
    let table_base = env
        .arr_proto_table_base
        .and_then(|global| global.get(&mut *caller).i32())
        .ok_or_else(|| wasmtime::Error::msg("missing __arr_proto_table_base global"))?
        as u32;
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
    crate::runtime_startup::install_array_proto_to_string(caller, &env, prototype)
        .ok_or_else(|| {
            wasmtime::Error::msg("V2 Array.prototype toString installation failed")
        })?;
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



fn take_next_handle(caller: &mut Caller<'_, RuntimeState>) -> wasmtime::Result<u32> {
    let env = crate::WasmEnv::from_caller(caller)
        .ok_or_else(|| wasmtime::Error::msg("missing cached WasmEnv"))?;
    let current = env
        .obj_table_count
        .get(&mut *caller)
        .i32()
        .ok_or_else(|| wasmtime::Error::msg("__obj_table_count is not i32"))?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| wasmtime::Error::msg("V2 handle table exhausted"))?;
    env.obj_table_count
        .set(&mut *caller, Val::I32(next))
        .map_err(host_error)?;
    Ok(current as u32)
}

fn host_error(error: impl std::fmt::Display) -> wasmtime::Error {
    wasmtime::Error::msg(error.to_string())
}
