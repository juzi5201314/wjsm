use anyhow::Result;
#[cfg(not(feature = "managed-heap-v2"))]
use wasmtime::Extern;
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
            #[cfg(feature = "managed-heap-v2")]
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
                return slot.value as i64;
            }
            #[cfg(not(feature = "managed-heap-v2"))]
            {
                let Some(ptr) = resolve_handle(&mut caller, obj) else {
                    return make_type_error_exception(
                        &mut caller,
                        "TypeError: Cannot read private member from a non-object",
                    );
                };
                let Some((slot_offset, flags, val)) =
                    find_private_property_slot_by_name_id(&mut caller, ptr, key_name_id as u32)
                else {
                    return make_type_error_exception(
                        &mut caller,
                        "TypeError: Cannot read private member from an object whose class did not declare it",
                    );
                };
                if (flags & constants::FLAG_IS_ACCESSOR) != 0 {
                    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                        return value::encode_undefined();
                    };
                    let data = memory.data(&caller);
                    if slot_offset + 24 > data.len() {
                        return value::encode_undefined();
                    }
                    let getter = i64::from_le_bytes(
                        data[slot_offset + 16..slot_offset + 24].try_into().unwrap(),
                    );
                    return invoke_private_accessor_get(&mut caller, getter, obj);
                }
                val
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
            #[cfg(feature = "managed-heap-v2")]
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
                return val;
            }
            #[cfg(not(feature = "managed-heap-v2"))]
            {
                let Some(ptr) = resolve_handle(&mut caller, obj) else {
                    return value::encode_undefined();
                };
                let found_slot =
                    find_private_property_slot_by_name_id(&mut caller, ptr, key_name_id as u32);
                if let Some((slot_offset, flags, _old_val)) = found_slot {
                    if (flags & constants::FLAG_IS_ACCESSOR) != 0 {
                        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                            return value::encode_undefined();
                        };
                        let data = memory.data(&caller);
                        if slot_offset + 32 > data.len() {
                            return value::encode_undefined();
                        }
                        let setter = i64::from_le_bytes(
                            data[slot_offset + 24..slot_offset + 32].try_into().unwrap(),
                        );
                        return invoke_private_accessor_set(&mut caller, setter, obj, val);
                    }
                    let Some(env) = WasmEnv::from_caller(&mut caller) else {
                        return value::encode_undefined();
                    };
                    let slot_idx = (slot_offset - (ptr + 16)) / 32;
                    let handle = handle_index_of(&mut caller, obj) as u32;
                    let _ = crate::runtime_gc::heap_access::write_property_slot(
                        &mut caller,
                        &env,
                        handle,
                        slot_idx,
                        crate::runtime_gc::heap_access::SlotPart::Value,
                        val,
                    );
                    val
                } else {
                    write_object_property_by_name_id(
                        &mut caller,
                        ptr,
                        obj,
                        key_name_id as u32,
                        val,
                        constants::FLAG_PRIVATE,
                    );
                    val
                }
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
            #[cfg(feature = "managed-heap-v2")]
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
                return obj;
            }
            #[cfg(not(feature = "managed-heap-v2"))]
            {
                let mut caller = caller;
                let Some(ptr) = resolve_handle(&mut caller, obj) else {
                    return value::encode_undefined();
                };
                write_private_accessor_slot(
                    &mut caller,
                    ptr,
                    obj,
                    key_name_id as u32,
                    getter,
                    setter,
                );
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
            #[cfg(feature = "managed-heap-v2")]
            {
                let Some(key_name_id) = private_property_key_v2(&mut caller, key_name_id as u32)
                else {
                    return value::encode_bool(false);
                };
                return value::encode_bool(
                    private_slot_v2(&mut caller, obj, key_name_id).is_some(),
                );
            }
            #[cfg(not(feature = "managed-heap-v2"))]
            {
                let Some(ptr) = resolve_handle(&mut caller, obj) else {
                    return value::encode_bool(false);
                };
                let found =
                    find_private_property_slot_by_name_id(&mut caller, ptr, key_name_id as u32);
                value::encode_bool(found.is_some())
            }
        },
    );
    linker.define(&mut store, "env", "private_has", private_has_fn)?;

    Ok(())
}

#[cfg(feature = "managed-heap-v2")]
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

#[cfg(feature = "managed-heap-v2")]
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
