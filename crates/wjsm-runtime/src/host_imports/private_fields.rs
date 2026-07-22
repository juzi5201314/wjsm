use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};
use wjsm_ir::{constants, value};

use crate::*;

/// 私有成员使用 FLAG_PRIVATE 槽存储，普通属性访问会跳过该槽。
pub(crate) fn define_private_fields(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let private_get_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32| -> i64 {
            if !value::is_js_object(obj) {
                return make_type_error_exception(
                    &mut caller,
                    "TypeError: Cannot read private member from a non-object",
                );
            }
            {
                let Some(key_name_id) = private_property_key_v2(&mut caller, key_name_id as u32)
                else {
                    return value::encode_undefined();
                };
                let Some(slot) = private_slot_v2(&mut caller, obj, key_name_id) else {
                    return make_type_error_exception(
                        &mut caller,
                        "TypeError: Cannot read private member from an object whose class did not declare it",
                    );
                };
                if slot.flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
                    return invoke_private_accessor_get(&mut caller, slot.getter as i64, obj);
                }
                slot.value as i64
            }
        },
    );
    linker.define(&mut store, "env", "private_get", private_get_fn)?;

    let private_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32, val: i64| -> i64 {
            if !value::is_js_object(obj) {
                set_runtime_error(
                    caller.data(),
                    "TypeError: cannot write private member to non-object".to_string(),
                );
                return value::encode_undefined();
            }
            {
                let Some(key_name_id) = private_property_key_v2(&mut caller, key_name_id as u32)
                else {
                    return value::encode_undefined();
                };
                let handle = value::decode_handle(obj);
                if let Some(slot) = private_slot_v2(&mut caller, obj, key_name_id) {
                    if slot.flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
                        return invoke_private_accessor_set(
                            &mut caller,
                            slot.setter as i64,
                            obj,
                            val,
                        );
                    }
                    if let Err(error) =
                        caller
                            .data()
                            .heap_access_v2()
                            .set_property(handle, key_name_id, val as u64)
                    {
                        set_runtime_error(
                            caller.data(),
                            format!("V2 private property write: {error}"),
                        );
                        return value::encode_undefined();
                    }
                    return val;
                }
                if let Err(error) = caller.data().heap_access_v2().define_data_property(
                    handle,
                    key_name_id,
                    val as u64,
                    constants::FLAG_PRIVATE as u32,
                ) {
                    set_runtime_error(
                        caller.data(),
                        format!("V2 private property define: {error}"),
                    );
                    return value::encode_undefined();
                }
                val
            }
        },
    );
    linker.define(&mut store, "env", "private_set", private_set_fn)?;

    let private_accessor_bind_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>,
         obj: i64,
         key_name_id: i32,
         getter: i64,
         setter: i64|
         -> i64 {
            if !value::is_js_object(obj) {
                set_runtime_error(
                    caller.data(),
                    "TypeError: cannot define private accessor on non-object".to_string(),
                );
                return value::encode_undefined();
            }
            {
                let mut caller = caller;
                let Some(key_name_id) = private_property_key_v2(&mut caller, key_name_id as u32)
                else {
                    return value::encode_undefined();
                };
                let handle = value::decode_handle(obj);
                if let Err(error) = caller
                    .data()
                    .heap_access_v2()
                    .define_accessor_property_with_flags(
                        handle,
                        key_name_id,
                        getter as u64,
                        setter as u64,
                        constants::FLAG_PRIVATE as u32,
                    )
                {
                    set_runtime_error(
                        caller.data(),
                        format!("V2 private accessor define: {error}"),
                    );
                    return value::encode_undefined();
                }
                obj
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "private_accessor_bind",
        private_accessor_bind_fn,
    )?;

    let private_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32| -> i64 {
            if !value::is_js_object(obj) {
                return value::encode_bool(false);
            }
            {
                let Some(key_name_id) = private_property_key_v2(&mut caller, key_name_id as u32)
                else {
                    return value::encode_bool(false);
                };
                value::encode_bool(private_slot_v2(&mut caller, obj, key_name_id).is_some())
            }
        },
    );
    linker.define(&mut store, "env", "private_has", private_has_fn)?;

    Ok(())
}

fn private_slot_v2(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    key_name_id: u32,
) -> Option<crate::runtime_gc::HeapAccessV2Property> {
    let handle = value::decode_handle(obj);
    match caller
        .data()
        .heap_access_v2()
        .get_property_slot(handle, key_name_id)
    {
        Ok(Some(slot)) if slot.flags & constants::FLAG_PRIVATE as u32 != 0 => Some(slot),
        Ok(_) => None,
        Err(error) => {
            set_runtime_error(
                caller.data(),
                format!("V2 private property lookup: {error}"),
            );
            None
        }
    }
}

fn private_property_key_v2(caller: &mut Caller<'_, RuntimeState>, name_id: u32) -> Option<u32> {
    crate::property_key::canonicalize_v2_name_id(caller, name_id)
}

fn invoke_private_accessor_get(
    caller: &mut Caller<'_, RuntimeState>,
    getter: i64,
    obj: i64,
) -> i64 {
    if value::is_undefined(getter) || value::is_null(getter) {
        return make_type_error_exception(
            caller,
            "TypeError: Cannot read private member without a getter",
        );
    }
    let rt = tokio::runtime::Handle::current();
    match tokio::task::block_in_place(|| {
        rt.block_on(crate::call_wasm_callback_async(caller, getter, obj, &[]))
    }) {
        Ok(value) => value,
        Err(error) => {
            set_runtime_error(
                caller.data(),
                format!("private accessor getter callback failed: {error:#}"),
            );
            value::encode_undefined()
        }
    }
}

fn invoke_private_accessor_set(
    caller: &mut Caller<'_, RuntimeState>,
    setter: i64,
    obj: i64,
    val: i64,
) -> i64 {
    if value::is_undefined(setter) || value::is_null(setter) {
        set_runtime_error(
            caller.data(),
            "TypeError: Cannot write private member without a setter".to_string(),
        );
        return value::encode_undefined();
    }
    let rt = tokio::runtime::Handle::current();
    match tokio::task::block_in_place(|| {
        rt.block_on(crate::call_wasm_callback_async(caller, setter, obj, &[val]))
    }) {
        Ok(value) => value,
        Err(error) => {
            set_runtime_error(
                caller.data(),
                format!("private accessor setter callback failed: {error:#}"),
            );
            value::encode_undefined()
        }
    }
}
