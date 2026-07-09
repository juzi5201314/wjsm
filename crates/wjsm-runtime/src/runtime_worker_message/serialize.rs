//! 源 agent 上的结构化序列化与 transfer detach。

use std::collections::{HashMap, HashSet};

use wasmtime::Caller;

use crate::runtime_buffer::visible_bytes;
use crate::runtime_render::read_runtime_string_utf8_lossy;
use crate::runtime_worker_message::{
    MESSAGE_PORT_ID_PROP, SAB_HANDLE_PROP, SerializedValue,
};
use crate::shared_buffer::read_sab_handle_from_object;
use crate::*;

struct SerCtx {
    visited: HashMap<i64, usize>,
    next_id: usize,
    /// transfer 列表中的对象身份（i64 句柄值）。
    transfer: HashSet<i64>,
    /// 已执行 transfer 的对象（防止二次 transfer）。
    transferred: HashSet<i64>,
}

/// 从数组参数解析 transfer list。
pub(crate) fn parse_transfer_list(
    caller: &mut Caller<'_, RuntimeState>,
    transfer_arg: i64,
) -> Result<Vec<i64>, String> {
    if value::is_undefined(transfer_arg) || value::is_null(transfer_arg) {
        return Ok(Vec::new());
    }
    if !value::is_array(transfer_arg) {
        return Err("transfer list must be an Array".to_string());
    }
    let Some(ptr) = resolve_array_ptr(caller, transfer_arg) else {
        return Err("transfer list must be an Array".to_string());
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let mut out = Vec::with_capacity(len as usize);
    for index in 0..len {
        let item = read_array_elem(caller, ptr, index).unwrap_or_else(value::encode_undefined);
        out.push(item);
    }
    Ok(out)
}

/// postMessage 入口：`value` + 可选 transfer 数组参数。
#[allow(dead_code)] // Worker postMessage 路径将调用。
pub(crate) fn serialize_for_post_message(
    caller: &mut Caller<'_, RuntimeState>,
    value: i64,
    transfer_arg: i64,
) -> Result<SerializedValue, String> {
    let transfer = parse_transfer_list(caller, transfer_arg)?;
    serialize_value(caller, value, &transfer)
}

/// 结构化序列化。`transfer_list` 中的 ArrayBuffer 会 detach 并迁走字节；
/// MessagePort 必须出现在 transfer 中；SAB 出现在 transfer 中会报错。
pub(crate) fn serialize_value(
    caller: &mut Caller<'_, RuntimeState>,
    value: i64,
    transfer_list: &[i64],
) -> Result<SerializedValue, String> {
    let mut transfer = HashSet::with_capacity(transfer_list.len());
    for &item in transfer_list {
        validate_transferable(caller, item)?;
        if !transfer.insert(item) {
            return Err("Transfer array contains duplicate entry".to_string());
        }
    }
    let mut cx = SerCtx {
        visited: HashMap::new(),
        next_id: 0,
        transfer,
        transferred: HashSet::new(),
    };
    let root = serialize_one(caller, value, &mut cx)?;
    // 未出现在值图中的 transfer 项仍需 detach / 标记。
    let pending: Vec<i64> = cx
        .transfer
        .iter()
        .copied()
        .filter(|v| !cx.transferred.contains(v))
        .collect();
    for item in pending {
        finalize_unseen_transfer(caller, item, &mut cx)?;
    }
    Ok(root)
}

/// 供 structuredClone 读取 options.transfer。
pub(crate) fn transfer_arg_from_options(
    caller: &mut Caller<'_, RuntimeState>,
    options: i64,
) -> Option<i64> {
    if !value::is_object(options) {
        return None;
    }
    let ptr = resolve_handle(caller, options)?;
    let transfer = read_object_property_by_name(caller, ptr, "transfer")?;
    if value::is_undefined(transfer) {
        return None;
    }
    Some(transfer)
}

fn validate_transferable(caller: &mut Caller<'_, RuntimeState>, item: i64) -> Result<(), String> {
    if read_sab_handle_from_object(caller, item).is_some() {
        return Err("SharedArrayBuffer can only be cloned, not transferred".to_string());
    }
    if message_port_id(caller, item).is_some() {
        return Ok(());
    }
    if arraybuffer_obj_handle(caller, item).is_some() {
        return Ok(());
    }
    Err("Value in transfer list is not transferable".to_string())
}

fn finalize_unseen_transfer(
    caller: &mut Caller<'_, RuntimeState>,
    item: i64,
    cx: &mut SerCtx,
) -> Result<(), String> {
    if cx.transferred.contains(&item) {
        return Ok(());
    }
    if message_port_id(caller, item).is_some() {
        cx.transferred.insert(item);
        return Ok(());
    }
    if arraybuffer_obj_handle(caller, item).is_some() {
        detach_arraybuffer(caller, item)?;
        cx.transferred.insert(item);
        return Ok(());
    }
    Ok(())
}

fn serialize_one(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
    cx: &mut SerCtx,
) -> Result<SerializedValue, String> {
    if value::is_undefined(value_raw) {
        return Ok(SerializedValue::Undefined);
    }
    if value::is_null(value_raw) {
        return Ok(SerializedValue::Null);
    }
    if value::is_bool(value_raw) {
        return Ok(SerializedValue::Bool(value::decode_bool(value_raw)));
    }
    if value::is_f64(value_raw) {
        return Ok(SerializedValue::Number(value::decode_f64(value_raw)));
    }
    if value::is_string(value_raw) || value::is_runtime_string_handle(value_raw) {
        return Ok(SerializedValue::String(read_runtime_string_utf8_lossy(
            caller, value_raw,
        )));
    }
    if value::is_bigint(value_raw) {
        return serialize_bigint(caller, value_raw);
    }
    if value::is_callable(value_raw) || value::is_native_callable(value_raw) {
        return Err("value could not be cloned".to_string());
    }
    if value::is_symbol(value_raw) {
        return Err("value could not be cloned".to_string());
    }
    if value::is_regexp(value_raw) {
        return serialize_regexp_handle(caller, value_raw, cx);
    }
    if value::is_array(value_raw) {
        return serialize_array(caller, value_raw, cx);
    }
    if !value::is_object(value_raw) {
        return Err("value could not be cloned".to_string());
    }
    if let Some(&id) = cx.visited.get(&value_raw) {
        return Ok(SerializedValue::Ref(id));
    }
    serialize_object_like(caller, value_raw, cx)
}

fn alloc_id(cx: &mut SerCtx, value_raw: i64) -> usize {
    let id = cx.next_id;
    cx.next_id += 1;
    cx.visited.insert(value_raw, id);
    id
}

fn serialize_bigint(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
) -> Result<SerializedValue, String> {
    let handle = value::decode_bigint_handle(value_raw) as usize;
    let table = caller
        .data()
        .bigint_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(bi) = table.get(handle) else {
        return Err("value could not be cloned".to_string());
    };
    Ok(SerializedValue::BigInt(bi.to_string()))
}

fn serialize_regexp_handle(
    caller: &mut Caller<'_, RuntimeState>,
    regexp: i64,
    cx: &mut SerCtx,
) -> Result<SerializedValue, String> {
    if let Some(&id) = cx.visited.get(&regexp) {
        return Ok(SerializedValue::Ref(id));
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
    let id = alloc_id(cx, regexp);
    Ok(SerializedValue::RegExp {
        id,
        source: entry.pattern,
        flags: entry.flags,
    })
}

fn serialize_array(
    caller: &mut Caller<'_, RuntimeState>,
    array: i64,
    cx: &mut SerCtx,
) -> Result<SerializedValue, String> {
    if let Some(&id) = cx.visited.get(&array) {
        return Ok(SerializedValue::Ref(id));
    }
    let Some(ptr) = resolve_array_ptr(caller, array) else {
        return Err("value could not be cloned".to_string());
    };
    let id = alloc_id(cx, array);
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let mut items = Vec::with_capacity(len as usize);
    for index in 0..len {
        let item = read_array_elem(caller, ptr, index).unwrap_or_else(value::encode_undefined);
        items.push(serialize_one(caller, item, cx)?);
    }
    Ok(SerializedValue::Array { id, items })
}

fn serialize_object_like(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    cx: &mut SerCtx,
) -> Result<SerializedValue, String> {
    // MessagePort：必须 transfer。
    if let Some(global_id) = message_port_id(caller, obj) {
        if !cx.transfer.contains(&obj) {
            return Err("MessagePort object must be listed in transfer".to_string());
        }
        if !cx.transferred.insert(obj) {
            return Err("MessagePort object has already been transferred".to_string());
        }
        let id = alloc_id(cx, obj);
        return Ok(SerializedValue::MessagePort { id, global_id });
    }
    // SharedArrayBuffer：共享 handle，不可 transfer。
    if let Some(handle) = read_sab_handle_from_object(caller, obj) {
        let id = alloc_id(cx, obj);
        return Ok(SerializedValue::SharedArrayBuffer { id, handle });
    }
    let Some(ptr) = resolve_handle(caller, obj) else {
        return Err("value could not be cloned".to_string());
    };
    if is_buffer_obj(caller, ptr) {
        let id = alloc_id(cx, obj);
        let bytes = visible_bytes(caller, obj).unwrap_or_default();
        return Ok(SerializedValue::Buffer { id, bytes });
    }
    if let Some(entry) = typedarray_entry_from_value(caller, obj) {
        let id = alloc_id(cx, obj);
        let bytes = visible_bytes(caller, obj).unwrap_or_default();
        return Ok(SerializedValue::TypedArray {
            id,
            kind: entry.element_kind,
            element_size: entry.element_size,
            bytes,
            byte_offset: entry.byte_offset,
            length: entry.length,
        });
    }
    if let Some((handle, len)) = arraybuffer_obj_handle(caller, obj) {
        let id = alloc_id(cx, obj);
        let do_transfer = cx.transfer.contains(&obj);
        let bytes = if do_transfer {
            if !cx.transferred.insert(obj) {
                return Err("ArrayBuffer has already been transferred".to_string());
            }
            take_arraybuffer_bytes(caller, handle, len)?
        } else {
            copy_arraybuffer_bytes(caller, handle, len)
        };
        if do_transfer {
            detach_arraybuffer_views(caller, obj, handle);
        }
        return Ok(SerializedValue::ArrayBuffer { id, bytes });
    }
    if let Some(ms) = date_ms(caller, ptr) {
        let id = alloc_id(cx, obj);
        return Ok(SerializedValue::Date { id, ms });
    }
    if read_object_property_by_name(caller, ptr, "__regexp_handle__").is_some()
        || (read_object_property_by_name(caller, ptr, "source").is_some()
            && read_object_property_by_name(caller, ptr, "flags").is_some())
    {
        let id = alloc_id(cx, obj);
        let source = prop_string(caller, ptr, "source");
        let flags = prop_string(caller, ptr, "flags");
        return Ok(SerializedValue::RegExp { id, source, flags });
    }
    if let Some(handle) = map_handle(caller, ptr) {
        return serialize_map(caller, obj, handle, cx);
    }
    if let Some(handle) = set_handle(caller, ptr) {
        return serialize_set(caller, obj, handle, cx);
    }
    serialize_plain_object(caller, obj, ptr, cx)
}

fn serialize_plain_object(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    ptr: usize,
    cx: &mut SerCtx,
) -> Result<SerializedValue, String> {
    let id = alloc_id(cx, obj);
    let names = collect_own_property_names_from_value(caller, obj, true);
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let Some(value_raw) = read_object_property_by_name(caller, ptr, &name) else {
            continue;
        };
        entries.push((name, serialize_one(caller, value_raw, cx)?));
    }
    Ok(SerializedValue::Object { id, entries })
}

fn serialize_map(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    handle: usize,
    cx: &mut SerCtx,
) -> Result<SerializedValue, String> {
    let id = alloc_id(cx, obj);
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
    let (keys, values) = snapshot;
    let mut entries = Vec::with_capacity(keys.len());
    for (key, val) in keys.into_iter().zip(values) {
        entries.push((
            serialize_one(caller, key, cx)?,
            serialize_one(caller, val, cx)?,
        ));
    }
    Ok(SerializedValue::Map { id, entries })
}

fn serialize_set(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    handle: usize,
    cx: &mut SerCtx,
) -> Result<SerializedValue, String> {
    let id = alloc_id(cx, obj);
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
    let mut values = Vec::with_capacity(snapshot.len());
    for val in snapshot {
        values.push(serialize_one(caller, val, cx)?);
    }
    Ok(SerializedValue::Set { id, values })
}

fn take_arraybuffer_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    handle: u32,
    len: u32,
) -> Result<Vec<u8>, String> {
    let mut table = caller
        .data()
        .arraybuffer_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(entry) = table.get_mut(handle as usize) else {
        return Err("value could not be cloned".to_string());
    };
    let bytes = if entry.data.len() >= len as usize {
        let taken = entry.data[..len as usize].to_vec();
        entry.data.clear();
        taken
    } else {
        std::mem::take(&mut entry.data)
    };
    Ok(bytes)
}

fn copy_arraybuffer_bytes(caller: &mut Caller<'_, RuntimeState>, handle: u32, len: u32) -> Vec<u8> {
    let table = caller
        .data()
        .arraybuffer_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    table
        .get(handle as usize)
        .and_then(|entry| entry.data.get(..len as usize).map(|s| s.to_vec()))
        .unwrap_or_default()
}

fn detach_arraybuffer(
    caller: &mut Caller<'_, RuntimeState>,
    ab_obj: i64,
) -> Result<(), String> {
    let Some((handle, _)) = arraybuffer_obj_handle(caller, ab_obj) else {
        return Err("Value in transfer list is not transferable".to_string());
    };
    {
        let mut table = caller
            .data()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(handle as usize) {
            entry.data.clear();
        }
    }
    detach_arraybuffer_views(caller, ab_obj, handle);
    Ok(())
}

fn detach_arraybuffer_views(caller: &mut Caller<'_, RuntimeState>, ab_obj: i64, handle: u32) {
    // byteLength 已存在：必须覆盖写入，不能只 define。
    set_existing_or_define(caller, ab_obj, "byteLength", value::encode_f64(0.0));
    let mut table = caller
        .data()
        .typedarray_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for entry in table.iter_mut() {
        if !entry.is_shared && entry.buffer_handle == handle {
            entry.byte_offset = 0;
            entry.length = 0;
            if let Some(view_obj) = entry.buffer_object {
                // buffer_object 是 AB 本身时已处理；view 长度在表内清零即可。
                let _ = view_obj;
            }
        }
    }
}

fn set_existing_or_define(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    val: i64,
) {
    let Some(obj_ptr) = resolve_handle(caller, obj) else {
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
        return;
    };
    let Some(name_id) = crate::find_memory_c_string(caller, name) else {
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
        return;
    };
    let flags =
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE;
    if find_property_slot_by_name_id(caller, obj_ptr, name_id).is_some() {
        write_object_property_by_name_id(caller, obj_ptr, obj, name_id, val, flags);
    } else {
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
    }
}

fn message_port_id(caller: &mut Caller<'_, RuntimeState>, obj: i64) -> Option<u32> {
    if !value::is_object(obj) {
        return None;
    }
    let ptr = resolve_handle(caller, obj)?;
    let raw = read_object_property_by_name(caller, ptr, MESSAGE_PORT_ID_PROP)?;
    if !value::is_f64(raw) {
        return None;
    }
    Some(value::decode_f64(raw) as u32)
}

fn arraybuffer_obj_handle(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
) -> Option<(u32, u32)> {
    if !value::is_object(obj) {
        return None;
    }
    let ptr = resolve_handle(caller, obj)?;
    if read_object_property_by_name(caller, ptr, "__typedarray_handle__").is_some() {
        return None;
    }
    if read_object_property_by_name(caller, ptr, SAB_HANDLE_PROP).is_some() {
        return None;
    }
    let handle = read_object_property_by_name(caller, ptr, "__arraybuffer_handle__")?;
    let byte_length = read_object_property_by_name(caller, ptr, "byteLength")?;
    Some((
        value::decode_f64(handle) as u32,
        value::decode_f64(byte_length) as u32,
    ))
}

fn is_buffer_obj(caller: &mut Caller<'_, RuntimeState>, ptr: usize) -> bool {
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

fn prop_string(caller: &mut Caller<'_, RuntimeState>, ptr: usize, name: &str) -> String {
    match read_object_property_by_name(caller, ptr, name) {
        Some(v) if value::is_string(v) || value::is_runtime_string_handle(v) => {
            read_runtime_string_utf8_lossy(caller, v)
        }
        _ => String::new(),
    }
}
