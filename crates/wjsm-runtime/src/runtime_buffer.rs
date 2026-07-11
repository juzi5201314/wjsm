use crate::runtime_encoding::{
    BufferEncoding, decode_bytes, encode_js_string, encoding_from_value,
};
use crate::*;

pub(crate) fn call_buffer_constructor(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let first = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_f64(first) {
        buffer_alloc(caller, args)
    } else {
        buffer_from(caller, args)
    }
}

pub(crate) fn call_buffer_static(
    caller: &mut Caller<'_, RuntimeState>,
    kind: BufferStaticKind,
    args: &[i64],
) -> i64 {
    match kind {
        BufferStaticKind::Alloc | BufferStaticKind::AllocUnsafe => buffer_alloc(caller, args),
        BufferStaticKind::From => buffer_from(caller, args),
        BufferStaticKind::Concat => buffer_concat(caller, args),
        BufferStaticKind::IsBuffer => value::encode_bool(is_buffer(
            caller,
            args.first()
                .copied()
                .unwrap_or_else(value::encode_undefined),
        )),
        BufferStaticKind::ByteLength => buffer_byte_length(caller, args),
    }
}

pub(crate) fn call_buffer_method(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: BufferMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        BufferMethodKind::ToString => buffer_to_string(caller, this_val, args),
        BufferMethodKind::Slice | BufferMethodKind::Subarray => {
            buffer_slice(caller, this_val, args)
        }
        BufferMethodKind::Copy => buffer_copy(caller, this_val, args),
        BufferMethodKind::Compare => buffer_compare(caller, this_val, args),
        BufferMethodKind::Write => buffer_write(caller, this_val, args),
        BufferMethodKind::ReadUInt8 => buffer_read_number(caller, this_val, args, NumberAccess::U8),
        BufferMethodKind::ReadUInt16BE => {
            buffer_read_number(caller, this_val, args, NumberAccess::U16BE)
        }
        BufferMethodKind::ReadUInt16LE => {
            buffer_read_number(caller, this_val, args, NumberAccess::U16LE)
        }
        BufferMethodKind::ReadUInt32BE => {
            buffer_read_number(caller, this_val, args, NumberAccess::U32BE)
        }
        BufferMethodKind::ReadUInt32LE => {
            buffer_read_number(caller, this_val, args, NumberAccess::U32LE)
        }
        BufferMethodKind::ReadInt8 => buffer_read_number(caller, this_val, args, NumberAccess::I8),
        BufferMethodKind::ReadInt16BE => {
            buffer_read_number(caller, this_val, args, NumberAccess::I16BE)
        }
        BufferMethodKind::ReadInt16LE => {
            buffer_read_number(caller, this_val, args, NumberAccess::I16LE)
        }
        BufferMethodKind::ReadInt32BE => {
            buffer_read_number(caller, this_val, args, NumberAccess::I32BE)
        }
        BufferMethodKind::ReadInt32LE => {
            buffer_read_number(caller, this_val, args, NumberAccess::I32LE)
        }
        BufferMethodKind::ReadFloatBE => {
            buffer_read_number(caller, this_val, args, NumberAccess::F32BE)
        }
        BufferMethodKind::ReadFloatLE => {
            buffer_read_number(caller, this_val, args, NumberAccess::F32LE)
        }
        BufferMethodKind::ReadDoubleBE => {
            buffer_read_number(caller, this_val, args, NumberAccess::F64BE)
        }
        BufferMethodKind::ReadDoubleLE => {
            buffer_read_number(caller, this_val, args, NumberAccess::F64LE)
        }
        BufferMethodKind::WriteUInt8 => {
            buffer_write_number(caller, this_val, args, NumberAccess::U8)
        }
        BufferMethodKind::WriteUInt16BE => {
            buffer_write_number(caller, this_val, args, NumberAccess::U16BE)
        }
        BufferMethodKind::WriteUInt16LE => {
            buffer_write_number(caller, this_val, args, NumberAccess::U16LE)
        }
        BufferMethodKind::WriteUInt32BE => {
            buffer_write_number(caller, this_val, args, NumberAccess::U32BE)
        }
        BufferMethodKind::WriteUInt32LE => {
            buffer_write_number(caller, this_val, args, NumberAccess::U32LE)
        }
        BufferMethodKind::WriteInt8 => {
            buffer_write_number(caller, this_val, args, NumberAccess::I8)
        }
        BufferMethodKind::WriteInt16BE => {
            buffer_write_number(caller, this_val, args, NumberAccess::I16BE)
        }
        BufferMethodKind::WriteInt16LE => {
            buffer_write_number(caller, this_val, args, NumberAccess::I16LE)
        }
        BufferMethodKind::WriteInt32BE => {
            buffer_write_number(caller, this_val, args, NumberAccess::I32BE)
        }
        BufferMethodKind::WriteInt32LE => {
            buffer_write_number(caller, this_val, args, NumberAccess::I32LE)
        }
        BufferMethodKind::WriteFloatBE => {
            buffer_write_number(caller, this_val, args, NumberAccess::F32BE)
        }
        BufferMethodKind::WriteFloatLE => {
            buffer_write_number(caller, this_val, args, NumberAccess::F32LE)
        }
        BufferMethodKind::WriteDoubleBE => {
            buffer_write_number(caller, this_val, args, NumberAccess::F64BE)
        }
        BufferMethodKind::WriteDoubleLE => {
            buffer_write_number(caller, this_val, args, NumberAccess::F64LE)
        }
        BufferMethodKind::Fill => buffer_fill(caller, this_val, args),
        BufferMethodKind::IndexOf => buffer_index_of(caller, this_val, args, false),
        BufferMethodKind::Includes => buffer_index_of(caller, this_val, args, true),
        BufferMethodKind::ToJson => buffer_to_json(caller, this_val),
        BufferMethodKind::Equals => buffer_equals(caller, this_val, args),
    }
}

pub(crate) fn is_buffer(caller: &mut Caller<'_, RuntimeState>, value_raw: i64) -> bool {
    let Some(ptr) = value::is_object(value_raw)
        .then(|| resolve_handle(caller, value_raw))
        .flatten()
    else {
        return false;
    };
    matches!(read_object_property_by_name(caller, ptr, "__buffer_brand__"), Some(v) if value::is_bool(v) && value::decode_bool(v))
}

pub(crate) fn create_buffer_from_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    bytes: Vec<u8>,
) -> i64 {
    let len = bytes.len() as u32;
    let ab_handle = {
        let mut table = caller
            .data()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(ArrayBufferEntry { data: bytes });
        handle
    };
    let ab_obj = create_arraybuffer_object(caller, ab_handle, len);
    create_buffer_view(caller, ab_handle, Some(ab_obj), 0, len)
}

/// 在 `Store`/`AsContextMut` 结算路径中创建 Buffer（不依赖 Caller 专用 API）。
pub(crate) fn create_buffer_from_bytes_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    bytes: Vec<u8>,
) -> i64 {
    let len = bytes.len() as u32;
    let ab_handle = {
        let mut table = ctx
            .as_context()
            .data()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(ArrayBufferEntry { data: bytes });
        handle
    };
    let ab_obj = {
        let obj = alloc_host_object(ctx, env, 2);
        let _ = define_host_data_property_with_env(
            ctx,
            env,
            obj,
            "__arraybuffer_handle__",
            value::encode_f64(ab_handle as f64),
        );
        let _ = define_host_data_property_with_env(
            ctx,
            env,
            obj,
            "byteLength",
            value::encode_f64(len as f64),
        );
        obj
    };
    let ta_handle = {
        let mut table = ctx
            .as_context()
            .data()
            .typedarray_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(TypedArrayEntry {
            buffer_handle: ab_handle,
            buffer_object: Some(ab_obj),
            byte_offset: 0,
            length: len,
            element_size: 1,
            element_kind: 1,
            is_shared: false,
        });
        handle
    };
    let obj = alloc_host_object(ctx, env, 7);
    if let Some(proto) = crate::runtime_heap::native_callable_buffer_prototype_with_env(ctx, env) {
        crate::runtime_heap::set_object_proto_header(ctx, env, obj, proto);
    }
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "__typedarray_handle__",
        value::encode_f64(ta_handle as f64),
    );
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "__arraybuffer_handle__",
        value::encode_f64(ab_handle as f64),
    );
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "__buffer_brand__",
        value::encode_bool(true),
    );
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "length",
        value::encode_f64(len as f64),
    );
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "byteLength",
        value::encode_f64(len as f64),
    );
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "byteOffset",
        value::encode_f64(0.0),
    );
    let _ = define_host_data_property_with_env(ctx, env, obj, "buffer", ab_obj);
    obj
}

pub(crate) fn create_arraybuffer_from_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    bytes: Vec<u8>,
) -> i64 {
    let len = bytes.len() as u32;
    let ab_handle = {
        let mut table = caller
            .data()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(ArrayBufferEntry { data: bytes });
        handle
    };
    create_arraybuffer_object(caller, ab_handle, len)
}

pub(crate) fn create_buffer_view(
    caller: &mut Caller<'_, RuntimeState>,
    buffer_handle: u32,
    buffer_object: Option<i64>,
    byte_offset: u32,
    length: u32,
) -> i64 {
    let ta_handle = {
        let mut table = caller
            .data()
            .typedarray_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(TypedArrayEntry {
            buffer_handle,
            buffer_object,
            byte_offset,
            length,
            element_size: 1,
            element_kind: 1,
            is_shared: false,
        });
        handle
    };
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 7);
    if let Some(proto) = crate::runtime_heap::native_callable_buffer_prototype(caller) {
        crate::runtime_heap::set_object_proto_header(caller, &env, obj, proto);
    }
    let buffer_object =
        buffer_object.unwrap_or_else(|| create_arraybuffer_object(caller, buffer_handle, length));
    define_buffer_prop(
        caller,
        obj,
        "__typedarray_handle__",
        value::encode_f64(ta_handle as f64),
    );
    define_buffer_prop(
        caller,
        obj,
        "__arraybuffer_handle__",
        value::encode_f64(buffer_handle as f64),
    );
    define_buffer_prop(caller, obj, "__buffer_brand__", value::encode_bool(true));
    define_buffer_prop(caller, obj, "length", value::encode_f64(length as f64));
    define_buffer_prop(caller, obj, "byteLength", value::encode_f64(length as f64));
    define_buffer_prop(
        caller,
        obj,
        "byteOffset",
        value::encode_f64(byte_offset as f64),
    );
    define_buffer_prop(caller, obj, "buffer", buffer_object);
    obj
}

pub(crate) fn visible_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
) -> Option<Vec<u8>> {
    let entry = typedarray_entry_from_value(caller, value_raw)?;
    read_entry_bytes(caller, &entry)
}

pub(crate) fn arraybuffer_visible_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
) -> Option<Vec<u8>> {
    let (handle, byte_length) = arraybuffer_handle(caller, value_raw)?;
    let table = caller.data().arraybuffer_table.lock().ok()?;
    let buffer = table.get(handle as usize)?;
    buffer
        .data
        .get(..byte_length as usize)
        .map(|bytes| bytes.to_vec())
}

fn define_buffer_prop(caller: &mut Caller<'_, RuntimeState>, obj: i64, name: &str, value_raw: i64) {
    let _ = define_host_data_property_from_caller(caller, obj, name, value_raw);
}

fn create_arraybuffer_object(
    caller: &mut Caller<'_, RuntimeState>,
    handle: u32,
    byte_length: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    define_buffer_prop(
        caller,
        obj,
        "__arraybuffer_handle__",
        value::encode_f64(handle as f64),
    );
    define_buffer_prop(
        caller,
        obj,
        "byteLength",
        value::encode_f64(byte_length as f64),
    );
    obj
}

fn buffer_alloc(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let size_arg = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let size = match buffer_size(caller, size_arg) {
        Ok(size) => size,
        Err(err) => return err,
    };
    let mut bytes = vec![0; size];
    if let Some(fill) = args.get(1).copied().filter(|v| !value::is_undefined(*v)) {
        let encoding = match optional_encoding(caller, args.get(2).copied()) {
            Ok(encoding) => encoding,
            Err(err) => return err,
        };
        let pattern = match fill_pattern(caller, fill, encoding) {
            Ok(pattern) => pattern,
            Err(err) => return err,
        };
        repeat_fill(&mut bytes, 0, size, &pattern);
    }
    create_buffer_from_bytes(caller, bytes)
}

fn buffer_from(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let first = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_string(first) || value::is_runtime_string_handle(first) {
        let encoding = match optional_encoding(caller, args.get(1).copied()) {
            Ok(encoding) => encoding,
            Err(err) => return err,
        };
        let bytes = match encode_js_string(caller, first, encoding) {
            Ok(bytes) => bytes,
            Err(label) => return unknown_encoding(caller, &label),
        };
        return create_buffer_from_bytes(caller, bytes);
    }
    if let Some((handle, byte_length)) = arraybuffer_handle(caller, first) {
        let offset = args
            .get(1)
            .copied()
            .filter(|v| !value::is_undefined(*v))
            .and_then(|v| to_usize(caller, v))
            .unwrap_or(0)
            .min(byte_length as usize);
        let default_len = byte_length as usize - offset;
        let length = args
            .get(2)
            .copied()
            .filter(|v| !value::is_undefined(*v))
            .and_then(|v| to_usize(caller, v))
            .unwrap_or(default_len)
            .min(default_len);
        return create_buffer_view(caller, handle, Some(first), offset as u32, length as u32);
    }
    if typedarray_entry_from_value(caller, first).is_some()
        && let Some(bytes) = visible_bytes(caller, first) {
            return create_buffer_from_bytes(caller, bytes);
        }
    if let Some(length) = array_like_length(caller, first) {
        let mut bytes = Vec::with_capacity(length as usize);
        for index in 0..length {
            let item = array_like_get(caller, first, index).unwrap_or_else(value::encode_undefined);
            bytes.push(to_byte(caller, item));
        }
        return create_buffer_from_bytes(caller, bytes);
    }
    make_type_error_exception(
        caller,
        "The first argument must be a string, Buffer, ArrayBuffer, Array, or Array-like object",
    )
}

fn buffer_concat(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let list = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(length) = array_like_length(caller, list) else {
        return make_type_error_exception(
            caller,
            "Buffer.concat list must be an array-like object",
        );
    };
    let mut parts = Vec::with_capacity(length as usize);
    let mut total = 0usize;
    for index in 0..length {
        let item = array_like_get(caller, list, index).unwrap_or_else(value::encode_undefined);
        let Some(bytes) = visible_bytes(caller, item) else {
            return make_type_error_exception(
                caller,
                "Buffer.concat list contains non-buffer value",
            );
        };
        total = total.saturating_add(bytes.len());
        parts.push(bytes);
    }
    let target_len = args
        .get(1)
        .copied()
        .filter(|v| !value::is_undefined(*v))
        .and_then(|v| to_usize(caller, v))
        .unwrap_or(total);
    let mut out = vec![0; target_len];
    let mut offset = 0usize;
    for part in parts {
        let count = part.len().min(target_len.saturating_sub(offset));
        if count == 0 {
            break;
        }
        out[offset..offset + count].copy_from_slice(&part[..count]);
        offset += count;
    }
    create_buffer_from_bytes(caller, out)
}

fn buffer_byte_length(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if let Some(bytes) = visible_bytes(caller, input) {
        return value::encode_f64(bytes.len() as f64);
    }
    if let Some((_, len)) = arraybuffer_handle(caller, input) {
        return value::encode_f64(len as f64);
    }
    let encoding = match optional_encoding(caller, args.get(1).copied()) {
        Ok(encoding) => encoding,
        Err(err) => return err,
    };
    match encode_js_string(caller, input, encoding) {
        Ok(bytes) => value::encode_f64(bytes.len() as f64),
        Err(label) => unknown_encoding(caller, &label),
    }
}

fn buffer_to_string(caller: &mut Caller<'_, RuntimeState>, this_val: i64, args: &[i64]) -> i64 {
    let Some(bytes) = visible_bytes(caller, this_val) else {
        return incompatible_buffer(caller);
    };
    let encoding = match optional_encoding(caller, args.first().copied()) {
        Ok(encoding) => encoding,
        Err(err) => return err,
    };
    let start = optional_offset(caller, args.get(1).copied(), bytes.len(), 0);
    let end = optional_offset(caller, args.get(2).copied(), bytes.len(), bytes.len()).max(start);
    decode_bytes(caller, &bytes[start..end.min(bytes.len())], encoding)
}

fn buffer_slice(caller: &mut Caller<'_, RuntimeState>, this_val: i64, args: &[i64]) -> i64 {
    let Some(entry) = typedarray_entry_from_value(caller, this_val) else {
        return incompatible_buffer(caller);
    };
    let len = entry.length as usize;
    let start = normalize_slice_arg(caller, args.first().copied(), len, 0);
    let end = normalize_slice_arg(caller, args.get(1).copied(), len, len).max(start);
    create_buffer_view(
        caller,
        entry.buffer_handle,
        entry.buffer_object,
        entry.byte_offset + start as u32,
        (end - start) as u32,
    )
}

fn buffer_copy(caller: &mut Caller<'_, RuntimeState>, this_val: i64, args: &[i64]) -> i64 {
    let Some(src) = visible_bytes(caller, this_val) else {
        return incompatible_buffer(caller);
    };
    let target = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(target_entry) = typedarray_entry_from_value(caller, target) else {
        return make_type_error_exception(
            caller,
            "Buffer.copy target must be a Buffer or Uint8Array",
        );
    };
    let target_start = optional_offset(
        caller,
        args.get(1).copied(),
        target_entry.length as usize,
        0,
    );
    let source_start = optional_offset(caller, args.get(2).copied(), src.len(), 0);
    let source_end =
        optional_offset(caller, args.get(3).copied(), src.len(), src.len()).max(source_start);
    let count = (source_end - source_start).min(target_entry.length as usize - target_start);
    write_entry_bytes(
        caller,
        &target_entry,
        target_start,
        &src[source_start..source_start + count],
    );
    value::encode_f64(count as f64)
}

fn buffer_compare(caller: &mut Caller<'_, RuntimeState>, this_val: i64, args: &[i64]) -> i64 {
    let Some(src) = visible_bytes(caller, this_val) else {
        return incompatible_buffer(caller);
    };
    let target = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(target_bytes) = visible_bytes(caller, target) else {
        return make_type_error_exception(
            caller,
            "Buffer.compare target must be a Buffer or Uint8Array",
        );
    };
    let target_start = optional_offset(caller, args.get(1).copied(), target_bytes.len(), 0);
    let target_end = optional_offset(
        caller,
        args.get(2).copied(),
        target_bytes.len(),
        target_bytes.len(),
    )
    .max(target_start);
    let source_start = optional_offset(caller, args.get(3).copied(), src.len(), 0);
    let source_end =
        optional_offset(caller, args.get(4).copied(), src.len(), src.len()).max(source_start);
    let ord = src[source_start..source_end.min(src.len())]
        .cmp(&target_bytes[target_start..target_end.min(target_bytes.len())]);
    value::encode_f64(match ord {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    })
}

fn buffer_write(caller: &mut Caller<'_, RuntimeState>, this_val: i64, args: &[i64]) -> i64 {
    let string = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(entry) = typedarray_entry_from_value(caller, this_val) else {
        return incompatible_buffer(caller);
    };
    let (offset, length, encoding) = match parse_write_args(caller, args, entry.length as usize) {
        Ok(parsed) => parsed,
        Err(err) => return err,
    };
    let bytes = match encode_js_string(caller, string, encoding) {
        Ok(bytes) => bytes,
        Err(label) => return unknown_encoding(caller, &label),
    };
    let count = bytes.len().min(length).min(entry.length as usize - offset);
    write_entry_bytes(caller, &entry, offset, &bytes[..count]);
    value::encode_f64(count as f64)
}

fn buffer_fill(caller: &mut Caller<'_, RuntimeState>, this_val: i64, args: &[i64]) -> i64 {
    let Some(entry) = typedarray_entry_from_value(caller, this_val) else {
        return incompatible_buffer(caller);
    };
    let value_raw = args
        .first()
        .copied()
        .unwrap_or_else(|| value::encode_f64(0.0));
    let offset = optional_offset(caller, args.get(1).copied(), entry.length as usize, 0);
    let end = optional_offset(
        caller,
        args.get(2).copied(),
        entry.length as usize,
        entry.length as usize,
    )
    .max(offset);
    let encoding_arg = if args.get(1).copied().is_some_and(value::is_string) {
        args.get(1).copied()
    } else if args.get(2).copied().is_some_and(value::is_string) {
        args.get(2).copied()
    } else {
        args.get(3).copied()
    };
    let encoding = match optional_encoding(caller, encoding_arg) {
        Ok(encoding) => encoding,
        Err(err) => return err,
    };
    let pattern = match fill_pattern(caller, value_raw, encoding) {
        Ok(pattern) => pattern,
        Err(err) => return err,
    };
    let mut bytes = read_entry_bytes(caller, &entry).unwrap_or_default();
    let capped_end = end.min(bytes.len());
    repeat_fill(&mut bytes, offset, capped_end, &pattern);
    write_entry_bytes(caller, &entry, offset, &bytes[offset..capped_end]);
    this_val
}

fn buffer_index_of(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: &[i64],
    as_bool: bool,
) -> i64 {
    let Some(bytes) = visible_bytes(caller, this_val) else {
        return incompatible_buffer(caller);
    };
    let needle_arg = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let start = normalize_slice_arg(caller, args.get(1).copied(), bytes.len(), 0).min(bytes.len());
    let encoding = match optional_encoding(caller, args.get(2).copied()) {
        Ok(encoding) => encoding,
        Err(err) => return err,
    };
    let needle = if value::is_f64(needle_arg) {
        vec![to_byte(caller, needle_arg)]
    } else if let Some(bytes) = visible_bytes(caller, needle_arg) {
        bytes
    } else {
        match encode_js_string(caller, needle_arg, encoding) {
            Ok(bytes) => bytes,
            Err(label) => return unknown_encoding(caller, &label),
        }
    };
    let found = find_subslice(&bytes[start..], &needle).map(|idx| idx + start);
    if as_bool {
        value::encode_bool(found.is_some())
    } else {
        value::encode_f64(found.map_or(-1.0, |idx| idx as f64))
    }
}

fn buffer_to_json(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> i64 {
    let Some(bytes) = visible_bytes(caller, this_val) else {
        return incompatible_buffer(caller);
    };
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let type_val = store_runtime_string(caller, "Buffer".to_string());
    define_buffer_prop(caller, obj, "type", type_val);
    let arr = alloc_array(caller, bytes.len() as u32);
    if let Some(ptr) = resolve_array_ptr(caller, arr) {
        for (index, byte) in bytes.iter().copied().enumerate() {
            write_array_elem(caller, ptr, index as u32, value::encode_f64(byte as f64));
        }
        write_array_length(caller, ptr, bytes.len() as u32);
    }
    define_buffer_prop(caller, obj, "data", arr);
    obj
}

fn buffer_equals(caller: &mut Caller<'_, RuntimeState>, this_val: i64, args: &[i64]) -> i64 {
    let Some(left) = visible_bytes(caller, this_val) else {
        return incompatible_buffer(caller);
    };
    let other = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(right) = visible_bytes(caller, other) else {
        return make_type_error_exception(
            caller,
            "Buffer.equals argument must be a Buffer or Uint8Array",
        );
    };
    value::encode_bool(left == right)
}

#[derive(Clone, Copy)]
enum NumberAccess {
    U8,
    U16BE,
    U16LE,
    U32BE,
    U32LE,
    I8,
    I16BE,
    I16LE,
    I32BE,
    I32LE,
    F32BE,
    F32LE,
    F64BE,
    F64LE,
}

impl NumberAccess {
    fn size(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16BE | Self::U16LE | Self::I16BE | Self::I16LE => 2,
            Self::U32BE | Self::U32LE | Self::I32BE | Self::I32LE | Self::F32BE | Self::F32LE => 4,
            Self::F64BE | Self::F64LE => 8,
        }
    }
}

fn buffer_read_number(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: &[i64],
    access: NumberAccess,
) -> i64 {
    let Some(bytes) = visible_bytes(caller, this_val) else {
        return incompatible_buffer(caller);
    };
    let offset = optional_offset(caller, args.first().copied(), bytes.len(), 0);
    let size = access.size();
    if offset.checked_add(size).is_none_or(|end| end > bytes.len()) {
        return make_range_error_exception(caller, "Index out of range");
    }
    let slice = &bytes[offset..offset + size];
    let number = match access {
        NumberAccess::U8 => slice[0] as f64,
        NumberAccess::I8 => slice[0] as i8 as f64,
        NumberAccess::U16BE => u16::from_be_bytes([slice[0], slice[1]]) as f64,
        NumberAccess::U16LE => u16::from_le_bytes([slice[0], slice[1]]) as f64,
        NumberAccess::I16BE => i16::from_be_bytes([slice[0], slice[1]]) as f64,
        NumberAccess::I16LE => i16::from_le_bytes([slice[0], slice[1]]) as f64,
        NumberAccess::U32BE => u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]) as f64,
        NumberAccess::U32LE => u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]) as f64,
        NumberAccess::I32BE => i32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]) as f64,
        NumberAccess::I32LE => i32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]) as f64,
        NumberAccess::F32BE => f32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]) as f64,
        NumberAccess::F32LE => f32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]) as f64,
        NumberAccess::F64BE => f64::from_be_bytes(slice.try_into().expect("8 bytes")),
        NumberAccess::F64LE => f64::from_le_bytes(slice.try_into().expect("8 bytes")),
    };
    value::encode_f64(number)
}

fn buffer_write_number(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: &[i64],
    access: NumberAccess,
) -> i64 {
    let Some(entry) = typedarray_entry_from_value(caller, this_val) else {
        return incompatible_buffer(caller);
    };
    let number = args
        .first()
        .copied()
        .map(|v| value::decode_f64(to_number(caller, v)))
        .unwrap_or(0.0);
    let offset = optional_offset(caller, args.get(1).copied(), entry.length as usize, 0);
    let size = access.size();
    if offset
        .checked_add(size)
        .is_none_or(|end| end > entry.length as usize)
    {
        return make_range_error_exception(caller, "Index out of range");
    }
    let bytes = match access {
        NumberAccess::U8 => vec![number as u8],
        NumberAccess::I8 => vec![number as i8 as u8],
        NumberAccess::U16BE => (number as u16).to_be_bytes().to_vec(),
        NumberAccess::U16LE => (number as u16).to_le_bytes().to_vec(),
        NumberAccess::I16BE => (number as i16).to_be_bytes().to_vec(),
        NumberAccess::I16LE => (number as i16).to_le_bytes().to_vec(),
        NumberAccess::U32BE => (number as u32).to_be_bytes().to_vec(),
        NumberAccess::U32LE => (number as u32).to_le_bytes().to_vec(),
        NumberAccess::I32BE => (number as i32).to_be_bytes().to_vec(),
        NumberAccess::I32LE => (number as i32).to_le_bytes().to_vec(),
        NumberAccess::F32BE => (number as f32).to_be_bytes().to_vec(),
        NumberAccess::F32LE => (number as f32).to_le_bytes().to_vec(),
        NumberAccess::F64BE => number.to_be_bytes().to_vec(),
        NumberAccess::F64LE => number.to_le_bytes().to_vec(),
    };
    write_entry_bytes(caller, &entry, offset, &bytes);
    value::encode_f64((offset + size) as f64)
}

fn buffer_size(caller: &mut Caller<'_, RuntimeState>, value_raw: i64) -> Result<usize, i64> {
    let n = value::decode_f64(to_number(caller, value_raw));
    if !n.is_finite() || n < 0.0 || n.trunc() > u32::MAX as f64 {
        return Err(make_range_error_exception(caller, "Invalid Buffer size"));
    }
    Ok(n.trunc() as usize)
}

fn optional_encoding(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
) -> Result<BufferEncoding, i64> {
    match value_raw.filter(|v| !value::is_undefined(*v)) {
        Some(v) => encoding_from_value(caller, v).map_err(|label| unknown_encoding(caller, &label)),
        None => Ok(BufferEncoding::Utf8),
    }
}

fn parse_write_args(
    caller: &mut Caller<'_, RuntimeState>,
    args: &[i64],
    buffer_len: usize,
) -> Result<(usize, usize, BufferEncoding), i64> {
    let mut offset = 0usize;
    let mut length = buffer_len;
    let mut encoding_arg = None;
    if let Some(second) = args.get(1).copied().filter(|v| !value::is_undefined(*v)) {
        if value::is_string(second) || value::is_runtime_string_handle(second) {
            encoding_arg = Some(second);
        } else {
            offset = optional_offset(caller, Some(second), buffer_len, 0);
            length = buffer_len.saturating_sub(offset);
        }
    }
    if let Some(third) = args.get(2).copied().filter(|v| !value::is_undefined(*v)) {
        if value::is_string(third) || value::is_runtime_string_handle(third) {
            encoding_arg = Some(third);
        } else {
            length = to_usize(caller, third)
                .unwrap_or(length)
                .min(buffer_len.saturating_sub(offset));
        }
    }
    if let Some(fourth) = args.get(3).copied().filter(|v| !value::is_undefined(*v)) {
        encoding_arg = Some(fourth);
    }
    let encoding = optional_encoding(caller, encoding_arg)?;
    Ok((
        offset.min(buffer_len),
        length.min(buffer_len.saturating_sub(offset)),
        encoding,
    ))
}

fn fill_pattern(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
    encoding: BufferEncoding,
) -> Result<Vec<u8>, i64> {
    let pattern = if value::is_f64(value_raw) {
        vec![to_byte(caller, value_raw)]
    } else if let Some(bytes) = visible_bytes(caller, value_raw) {
        bytes
    } else {
        encode_js_string(caller, value_raw, encoding)
            .map_err(|label| unknown_encoding(caller, &label))?
    };
    Ok(if pattern.is_empty() { vec![0] } else { pattern })
}

fn repeat_fill(bytes: &mut [u8], start: usize, end: usize, pattern: &[u8]) {
    if pattern.is_empty() || start >= end || start >= bytes.len() {
        return;
    }
    let end = end.min(bytes.len());
    for index in start..end {
        bytes[index] = pattern[(index - start) % pattern.len()];
    }
}

fn read_entry_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    entry: &TypedArrayEntry,
) -> Option<Vec<u8>> {
    let start = entry.byte_offset as usize;
    let end = start.checked_add(entry.length as usize * entry.element_size as usize)?;
    if entry.is_shared {
        let shared = caller.data().shared_state.as_ref()?.clone();
        let table = shared.sab_table.lock().ok()?;
        let buffer = table.get(entry.buffer_handle as usize)?;
        let data = buffer.data.read().ok()?;
        data.get(start..end).map(|slice| slice.to_vec())
    } else {
        let table = caller.data().arraybuffer_table.lock().ok()?;
        let buffer = table.get(entry.buffer_handle as usize)?;
        buffer.data.get(start..end).map(|slice| slice.to_vec())
    }
}

pub(crate) fn write_entry_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    entry: &TypedArrayEntry,
    offset: usize,
    bytes: &[u8],
) -> bool {
    let start = entry.byte_offset as usize + offset;
    let Some(end) = start.checked_add(bytes.len()) else {
        return false;
    };
    if entry.is_shared {
        let Some(shared) = caller.data().shared_state.as_ref().cloned() else {
            return false;
        };
        let Ok(table) = shared.sab_table.lock() else {
            return false;
        };
        let Some(buffer) = table.get(entry.buffer_handle as usize) else {
            return false;
        };
        let Ok(mut data) = buffer.data.write() else {
            return false;
        };
        if end > data.len() {
            return false;
        }
        data[start..end].copy_from_slice(bytes);
        true
    } else {
        let Ok(mut table) = caller.data().arraybuffer_table.lock() else {
            return false;
        };
        let Some(buffer) = table.get_mut(entry.buffer_handle as usize) else {
            return false;
        };
        if end > buffer.data.len() {
            return false;
        }
        buffer.data[start..end].copy_from_slice(bytes);
        true
    }
}

fn arraybuffer_handle(caller: &mut Caller<'_, RuntimeState>, value_raw: i64) -> Option<(u32, u32)> {
    if !value::is_object(value_raw) {
        return None;
    }
    let ptr = resolve_handle(caller, value_raw)?;
    let handle = read_object_property_by_name(caller, ptr, "__arraybuffer_handle__")?;
    let byte_length = read_object_property_by_name(caller, ptr, "byteLength")?;
    Some((
        value::decode_f64(handle) as u32,
        value::decode_f64(byte_length) as u32,
    ))
}

fn array_like_length(caller: &mut Caller<'_, RuntimeState>, value_raw: i64) -> Option<u32> {
    if value::is_array(value_raw) {
        let ptr = resolve_array_ptr(caller, value_raw)?;
        return read_array_length(caller, ptr);
    }
    if value::is_object(value_raw) {
        let ptr = resolve_handle(caller, value_raw)?;
        let len = read_object_property_by_name(caller, ptr, "length")?;
        return Some(to_usize(caller, len).unwrap_or(0).min(u32::MAX as usize) as u32);
    }
    None
}

fn array_like_get(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
    index: u32,
) -> Option<i64> {
    if value::is_array(value_raw) {
        let ptr = resolve_array_ptr(caller, value_raw)?;
        return read_array_elem(caller, ptr, index);
    }
    if value::is_object(value_raw) {
        let ptr = resolve_handle(caller, value_raw)?;
        return read_object_property_by_name(caller, ptr, &index.to_string());
    }
    None
}

fn to_usize(caller: &mut Caller<'_, RuntimeState>, value_raw: i64) -> Option<usize> {
    let n = value::decode_f64(to_number(caller, value_raw));
    if n.is_nan() || n <= 0.0 {
        return Some(0);
    }
    n.is_finite()
        .then(|| n.trunc().min(u32::MAX as f64) as usize)
}

fn to_byte(caller: &mut Caller<'_, RuntimeState>, value_raw: i64) -> u8 {
    let n = value::decode_f64(to_number(caller, value_raw));
    if !n.is_finite() || n.is_nan() || n == 0.0 {
        0
    } else {
        n.trunc().rem_euclid(256.0) as u8
    }
}

fn optional_offset(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
    len: usize,
    default_value: usize,
) -> usize {
    match value_raw.filter(|v| !value::is_undefined(*v)) {
        Some(v) => to_usize(caller, v).unwrap_or(default_value).min(len),
        None => default_value.min(len),
    }
}

fn normalize_slice_arg(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
    len: usize,
    default_value: usize,
) -> usize {
    let Some(v) = value_raw.filter(|v| !value::is_undefined(*v)) else {
        return default_value.min(len);
    };
    let n = value::decode_f64(to_number(caller, v));
    if n.is_nan() {
        return 0;
    }
    let idx = if n < 0.0 {
        len as f64 + n.trunc()
    } else {
        n.trunc()
    };
    idx.max(0.0).min(len as f64) as usize
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn unknown_encoding(caller: &mut Caller<'_, RuntimeState>, label: &str) -> i64 {
    make_type_error_exception(caller, &format!("Unknown encoding: {label}"))
}

fn incompatible_buffer(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    make_type_error_exception(caller, "Method called on incompatible Buffer receiver")
}
