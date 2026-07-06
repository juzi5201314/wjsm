use std::collections::HashMap;

use crate::runtime_buffer::{
    create_arraybuffer_from_bytes, create_buffer_from_bytes, visible_bytes,
};
use crate::*;

pub(crate) fn structured_clone(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    if let Some(options) = args.get(1).copied()
        && has_transfer(caller, options)
    {
        return data_clone_error(caller, "transfer is not supported");
    }
    let value_raw = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let mut cx = CloneContext::default();
    clone_value(caller, value_raw, &mut cx).unwrap_or_else(|msg| data_clone_error(caller, &msg))
}

#[derive(Default)]
struct CloneContext {
    visited: HashMap<i64, i64>,
}

fn clone_value(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
    cx: &mut CloneContext,
) -> Result<i64, String> {
    if value::is_undefined(value_raw)
        || value::is_null(value_raw)
        || value::is_bool(value_raw)
        || value::is_f64(value_raw)
        || value::is_bigint(value_raw)
        || value::is_string(value_raw)
        || value::is_runtime_string_handle(value_raw)
    {
        return Ok(value_raw);
    }
    if value::is_callable(value_raw) || value::is_native_callable(value_raw) {
        return Err("value could not be cloned".to_string());
    }
    if value::is_regexp(value_raw) {
        return clone_regexp_handle(caller, value_raw, cx);
    }
    if value::is_array(value_raw) {
        return clone_array(caller, value_raw, cx);
    }
    if !value::is_object(value_raw) {
        return Err("value could not be cloned".to_string());
    }
    if let Some(existing) = cx.visited.get(&value_raw).copied() {
        return Ok(existing);
    }
    clone_object_like(caller, value_raw, cx)
}

fn clone_array(
    caller: &mut Caller<'_, RuntimeState>,
    array: i64,
    cx: &mut CloneContext,
) -> Result<i64, String> {
    if let Some(existing) = cx.visited.get(&array).copied() {
        return Ok(existing);
    }
    let Some(ptr) = resolve_array_ptr(caller, array) else {
        return Err("value could not be cloned".to_string());
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let clone = alloc_array(caller, len);
    let clone_ptr = resolve_array_ptr(caller, clone).unwrap_or(ptr);
    cx.visited.insert(array, clone);
    for index in 0..len {
        if let Some(item) = read_array_elem(caller, ptr, index) {
            let cloned = clone_value(caller, item, cx)?;
            write_array_elem(caller, clone_ptr, index, cloned);
        }
    }
    write_array_length(caller, clone_ptr, len);
    Ok(clone)
}

fn clone_object_like(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    cx: &mut CloneContext,
) -> Result<i64, String> {
    let Some(ptr) = resolve_handle(caller, obj) else {
        return Err("value could not be cloned".to_string());
    };
    if is_buffer(caller, ptr) {
        let bytes = visible_bytes(caller, obj).unwrap_or_default();
        let clone = create_buffer_from_bytes(caller, bytes);
        cx.visited.insert(obj, clone);
        return Ok(clone);
    }
    if typedarray_entry_from_value(caller, obj).is_some() {
        let bytes = visible_bytes(caller, obj).unwrap_or_default();
        let clone = create_buffer_from_bytes(caller, bytes);
        cx.visited.insert(obj, clone);
        return Ok(clone);
    }
    if let Some((handle, len)) = arraybuffer_handle(caller, ptr) {
        let bytes = {
            let table = caller
                .data()
                .arraybuffer_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table
                .get(handle as usize)
                .and_then(|entry| entry.data.get(..len as usize).map(|slice| slice.to_vec()))
                .unwrap_or_default()
        };
        let clone = create_arraybuffer_from_bytes(caller, bytes);
        cx.visited.insert(obj, clone);
        return Ok(clone);
    }
    if let Some(ms) = date_ms(caller, ptr) {
        let clone = clone_date(caller, ms);
        cx.visited.insert(obj, clone);
        return Ok(clone);
    }
    if read_object_property_by_name(caller, ptr, "__regexp_handle__").is_some() {
        let clone = clone_regexp_plain(caller, ptr);
        cx.visited.insert(obj, clone);
        return Ok(clone);
    }
    if let Some(handle) = map_handle(caller, ptr) {
        return clone_map(caller, obj, handle, cx);
    }
    if let Some(handle) = set_handle(caller, ptr) {
        return clone_set(caller, obj, handle, cx);
    }
    clone_plain_object(caller, obj, ptr, cx)
}

fn clone_plain_object(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    ptr: usize,
    cx: &mut CloneContext,
) -> Result<i64, String> {
    let names = collect_own_property_names_from_value(caller, obj, true);
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let clone = alloc_host_object(caller, &env, names.len() as u32);
    cx.visited.insert(obj, clone);
    for name in names {
        let Some(value_raw) = read_object_property_by_name(caller, ptr, &name) else {
            continue;
        };
        let cloned = clone_value(caller, value_raw, cx)?;
        let _ = define_host_data_property_from_caller(caller, clone, &name, cloned);
    }
    Ok(clone)
}

fn clone_map(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    handle: usize,
    cx: &mut CloneContext,
) -> Result<i64, String> {
    let snapshot = {
        let table = caller
            .data()
            .map_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle)
            .map(|entry| (entry.keys.clone(), entry.values.clone()))
            .unwrap_or_default()
    };
    let new_handle = caller.data().alloc_map_entry();
    let clone = create_map_shell(caller, new_handle);
    cx.visited.insert(obj, clone);
    let (keys, values) = snapshot;
    let mut cloned_keys = Vec::with_capacity(keys.len());
    let mut cloned_values = Vec::with_capacity(values.len());
    for (key, value_raw) in keys.into_iter().zip(values) {
        cloned_keys.push(clone_value(caller, key, cx)?);
        cloned_values.push(clone_value(caller, value_raw, cx)?);
    }
    {
        let mut table = caller
            .data()
            .map_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(new_handle as usize) {
            entry.keys = cloned_keys;
            entry.values = cloned_values;
        }
    }
    Ok(clone)
}

fn clone_set(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    handle: usize,
    cx: &mut CloneContext,
) -> Result<i64, String> {
    let snapshot = {
        let table = caller
            .data()
            .set_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle)
            .map(|entry| entry.values.clone())
            .unwrap_or_default()
    };
    let new_handle = caller.data().alloc_set_entry();
    let clone = create_set_shell(caller, new_handle);
    cx.visited.insert(obj, clone);
    let mut cloned_values = Vec::with_capacity(snapshot.len());
    for value_raw in snapshot {
        cloned_values.push(clone_value(caller, value_raw, cx)?);
    }
    {
        let mut table = caller
            .data()
            .set_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(new_handle as usize) {
            entry.values = cloned_values;
        }
    }
    Ok(clone)
}

fn create_map_shell(caller: &mut Caller<'_, RuntimeState>, handle: u32) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    caller
        .data()
        .bind_map_entry_owner(handle, value::decode_object_handle(obj));
    let size_fn = create_map_set_method(caller.data(), MapSetMethodKind::Size);
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "__map_handle__",
        value::encode_f64(handle as f64),
    );
    let _ = define_host_accessor_property_from_caller(
        caller,
        obj,
        "size",
        size_fn,
        value::encode_undefined(),
    );
    obj
}

fn create_set_shell(caller: &mut Caller<'_, RuntimeState>, handle: u32) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    caller
        .data()
        .bind_set_entry_owner(handle, value::decode_object_handle(obj));
    let size_fn = create_map_set_method(caller.data(), MapSetMethodKind::Size);
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "__set_handle__",
        value::encode_f64(handle as f64),
    );
    let _ = define_host_accessor_property_from_caller(
        caller,
        obj,
        "size",
        size_fn,
        value::encode_undefined(),
    );
    obj
}

fn clone_regexp_handle(
    caller: &mut Caller<'_, RuntimeState>,
    regexp: i64,
    cx: &mut CloneContext,
) -> Result<i64, String> {
    if let Some(existing) = cx.visited.get(&regexp).copied() {
        return Ok(existing);
    }
    let handle = value::decode_regexp_handle(regexp) as usize;
    let entry = {
        let table = caller
            .data()
            .regex_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table.get(handle).cloned()
    };
    let Some(entry) = entry else {
        return Err("value could not be cloned".to_string());
    };
    let clone = {
        let mut table = caller
            .data()
            .regex_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let new_handle = table.len() as u32;
        table.push(entry);
        value::encode_regexp_handle(new_handle)
    };
    cx.visited.insert(regexp, clone);
    Ok(clone)
}

fn clone_date(caller: &mut Caller<'_, RuntimeState>, ms: f64) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let get_time = create_date_method(caller.data(), DateMethodKind::GetTime);
    let _ =
        define_host_data_property_from_caller(caller, obj, "__date_ms__", value::encode_f64(ms));
    let _ = define_host_data_property_from_caller(caller, obj, "getTime", get_time);
    obj
}

fn clone_regexp_plain(caller: &mut Caller<'_, RuntimeState>, ptr: usize) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let source = read_object_property_by_name(caller, ptr, "source")
        .unwrap_or_else(|| store_runtime_string(caller, String::new()));
    let flags = read_object_property_by_name(caller, ptr, "flags")
        .unwrap_or_else(|| store_runtime_string(caller, String::new()));
    let _ = define_host_data_property_from_caller(caller, obj, "source", source);
    let _ = define_host_data_property_from_caller(caller, obj, "flags", flags);
    obj
}

fn is_buffer(caller: &mut Caller<'_, RuntimeState>, ptr: usize) -> bool {
    read_object_property_by_name(caller, ptr, "__buffer_brand__")
        .is_some_and(|v| value::is_bool(v) && value::decode_bool(v))
}

fn date_ms(caller: &mut Caller<'_, RuntimeState>, ptr: usize) -> Option<f64> {
    read_object_property_by_name(caller, ptr, "__date_ms__").map(value::decode_f64)
}

fn map_handle(caller: &mut Caller<'_, RuntimeState>, ptr: usize) -> Option<usize> {
    read_object_property_by_name(caller, ptr, "__map_handle__")
        .map(|v| value::decode_f64(v) as usize)
}

fn set_handle(caller: &mut Caller<'_, RuntimeState>, ptr: usize) -> Option<usize> {
    read_object_property_by_name(caller, ptr, "__set_handle__")
        .map(|v| value::decode_f64(v) as usize)
}

fn arraybuffer_handle(caller: &mut Caller<'_, RuntimeState>, ptr: usize) -> Option<(u32, u32)> {
    if read_object_property_by_name(caller, ptr, "__typedarray_handle__").is_some() {
        return None;
    }
    let handle = read_object_property_by_name(caller, ptr, "__arraybuffer_handle__")?;
    let byte_length = read_object_property_by_name(caller, ptr, "byteLength")?;
    Some((
        value::decode_f64(handle) as u32,
        value::decode_f64(byte_length) as u32,
    ))
}

fn has_transfer(caller: &mut Caller<'_, RuntimeState>, options: i64) -> bool {
    if !value::is_object(options) {
        return false;
    }
    let Some(ptr) = resolve_handle(caller, options) else {
        return false;
    };
    read_object_property_by_name(caller, ptr, "transfer").is_some_and(|v| !value::is_undefined(v))
}

fn data_clone_error(caller: &mut Caller<'_, RuntimeState>, message: &str) -> i64 {
    let msg_val = store_runtime_string(caller, message.to_string());
    let error_obj =
        create_error_object(caller, "DataCloneError", msg_val, value::encode_undefined());
    let mut errors = caller
        .data()
        .error_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let idx = errors.len() as u32;
    errors.push(ErrorEntry {
        name: "DataCloneError".to_string(),
        message: message.to_string(),
        value: error_obj,
    });
    value::encode_handle(value::TAG_EXCEPTION, idx)
}
