//! TypedArray 元素编解码和构造操作
//!
//! 从 lib.rs 抽取的 TypedArray 相关函数，包括：
//! - 元素编解码（decode/encode）
//! - 构造（construct）
//! - 元素读写（element_read/element_write）
//! - 辅助函数（bigint 处理、类型转换等）

use super::*;

pub(crate) fn bigint_low_64_bytes(value: &num_bigint::BigInt) -> [u8; 8] {
    let fill = if value.sign() == num_bigint::Sign::Minus {
        0xff
    } else {
        0
    };
    let mut out = [fill; 8];
    let bytes = value.to_signed_bytes_le();
    let len = bytes.len().min(8);
    out[..len].copy_from_slice(&bytes[..len]);
    out
}
pub(crate) fn typedarray_entry_from_value(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
) -> Option<TypedArrayEntry> {
    if !value::is_object(value_raw) {
        return None;
    }
    let ptr = resolve_handle(caller, value_raw)?;
    let handle_raw = read_object_property_by_name(caller, ptr, "__typedarray_handle__")?;
    let handle = value::decode_f64(handle_raw) as usize;
    let table = caller.data().typedarray_table.lock().ok()?;
    table.get(handle).cloned()
}

/// 通用 AsContextMut 版本的 typed-array entry 查询。
/// 与 typedarray_entry_from_value 语义一致，但可用于 Store/Caller 等任意 context。
pub(crate) fn typedarray_entry_from_value_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    value_raw: i64,
) -> Option<TypedArrayEntry> {
    if !value::is_object(value_raw) {
        return None;
    }
    let handle_idx = (value_raw as u64 & 0xFFFF_FFFF) as usize;
    let ptr = resolve_handle_idx_with_env(ctx, env, handle_idx)?;
    let handle_raw = read_object_property_by_name_with_env(ctx, env, ptr, "__typedarray_handle__")?;
    let handle = value::decode_f64(handle_raw) as usize;
    let store = ctx.as_context_mut();
    let table = store.data().typedarray_table.lock().ok()?;
    table.get(handle).cloned()
}

pub(crate) fn typedarray_element_offset(entry: &TypedArrayEntry, index: u32) -> Option<usize> {
    if index >= entry.length {
        return None;
    }
    Some(entry.byte_offset as usize + index as usize * entry.element_size as usize)
}

pub(crate) fn decode_typedarray_element(
    caller: &mut Caller<'_, RuntimeState>,
    bytes: &[u8; 8],
    elem_size: u8,
    element_kind: u8,
) -> Option<i64> {
    let value = match (elem_size, element_kind) {
        (1, 0) => value::encode_f64(bytes[0] as i8 as f64),
        (1, 1) | (1, 2) => value::encode_f64(bytes[0] as f64),
        (2, 0) => value::encode_f64(i16::from_le_bytes([bytes[0], bytes[1]]) as f64),
        (2, 1) => value::encode_f64(u16::from_le_bytes([bytes[0], bytes[1]]) as f64),
        (4, 0) => {
            value::encode_f64(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64)
        }
        (4, 1) => {
            value::encode_f64(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64)
        }
        (4, 3) => {
            value::encode_f64(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64)
        }
        (8, 3) => value::encode_f64(f64::from_le_bytes(*bytes)),
        (8, 4) => {
            let raw = i64::from_le_bytes(*bytes);
            let mut table = caller.data().bigint_table.lock().ok()?;
            let handle = table.len() as u32;
            table.push(num_bigint::BigInt::from(raw));
            value::encode_bigint_handle(handle)
        }
        (8, 5) => {
            let raw = u64::from_le_bytes(*bytes);
            let mut table = caller.data().bigint_table.lock().ok()?;
            let handle = table.len() as u32;
            table.push(num_bigint::BigInt::from(raw));
            value::encode_bigint_handle(handle)
        }
        _ => return None,
    };
    Some(value)
}

pub(crate) fn to_uint8_clamp(number: f64) -> u8 {
    if number.is_nan() || number <= 0.0 {
        return 0;
    }
    if number >= 255.0 {
        return 255;
    }
    let floor = number.floor();
    let delta = number - floor;
    if delta > 0.5 {
        return floor as u8 + 1;
    }
    if delta < 0.5 {
        return floor as u8;
    }
    let value = floor as u8;
    value + (value & 1)
}

pub(crate) fn set_typedarray_runtime_error(
    caller: &mut Caller<'_, RuntimeState>,
    message: &'static str,
) {
    set_runtime_error(caller.data(), message.to_string());
}

pub(crate) fn typedarray_to_number(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
) -> Option<f64> {
    if value::is_bigint(value_raw) {
        set_typedarray_runtime_error(
            caller,
            "TypeError: Cannot convert a BigInt value to a number",
        );
        return None;
    }
    let number_raw = to_number(caller, value_raw);
    if caller
        .data()
        .runtime_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
    {
        return None;
    }
    Some(value::decode_f64(number_raw))
}

pub(crate) fn to_uint_n(number: f64, bits: u32) -> u32 {
    if number == 0.0 || !number.is_finite() {
        return 0;
    }
    let modulo = 2.0_f64.powi(bits as i32);
    number.trunc().rem_euclid(modulo) as u32
}

pub(crate) fn to_int_n(number: f64, bits: u32) -> i32 {
    let unsigned = to_uint_n(number, bits);
    let sign_bit = 1u32 << (bits - 1);
    if (unsigned & sign_bit) == 0 {
        unsigned as i32
    } else {
        (unsigned as i64 - (1i64 << bits)) as i32
    }
}

pub(crate) fn typedarray_to_index(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
    range_error: &'static str,
) -> Option<u32> {
    if value::is_undefined(value_raw) {
        return Some(0);
    }
    let number = typedarray_to_number(caller, value_raw)?;
    if number.is_nan() || number == 0.0 {
        return Some(0);
    }
    if !number.is_finite() || number < 0.0 || number.trunc() > u32::MAX as f64 {
        set_typedarray_runtime_error(caller, range_error);
        return None;
    }
    Some(number.trunc() as u32)
}

pub(crate) fn typedarray_byte_len(
    caller: &mut Caller<'_, RuntimeState>,
    len: u32,
    elem_size: u32,
) -> Option<usize> {
    let Some(byte_len) = len.checked_mul(elem_size) else {
        set_typedarray_runtime_error(caller, "RangeError: Invalid typed array length");
        return None;
    };
    Some(byte_len as usize)
}

pub(crate) fn encode_typedarray_element(
    caller: &mut Caller<'_, RuntimeState>,
    elem_size: u8,
    element_kind: u8,
    value_raw: i64,
) -> Option<[u8; 8]> {
    let mut out = [0u8; 8];
    match (elem_size, element_kind) {
        (1, 0) => out[0] = to_int_n(typedarray_to_number(caller, value_raw)?, 8) as i8 as u8,
        (1, 1) => out[0] = to_uint_n(typedarray_to_number(caller, value_raw)?, 8) as u8,
        (1, 2) => out[0] = to_uint8_clamp(typedarray_to_number(caller, value_raw)?),
        (2, 0) => out[..2].copy_from_slice(
            &(to_int_n(typedarray_to_number(caller, value_raw)?, 16) as i16).to_le_bytes(),
        ),
        (2, 1) => out[..2].copy_from_slice(
            &(to_uint_n(typedarray_to_number(caller, value_raw)?, 16) as u16).to_le_bytes(),
        ),
        (4, 0) => out[..4]
            .copy_from_slice(&to_int_n(typedarray_to_number(caller, value_raw)?, 32).to_le_bytes()),
        (4, 1) => out[..4].copy_from_slice(
            &to_uint_n(typedarray_to_number(caller, value_raw)?, 32).to_le_bytes(),
        ),
        (4, 3) => out[..4]
            .copy_from_slice(&(typedarray_to_number(caller, value_raw)? as f32).to_le_bytes()),
        (8, 3) => out.copy_from_slice(&typedarray_to_number(caller, value_raw)?.to_le_bytes()),
        (8, 4) | (8, 5) => {
            if !value::is_bigint(value_raw) {
                set_typedarray_runtime_error(caller, "TypeError: Cannot convert value to a BigInt");
                return None;
            }
            let handle = value::decode_bigint_handle(value_raw) as usize;
            let table = caller.data().bigint_table.lock().ok()?;
            let bigint = table.get(handle)?;
            out = bigint_low_64_bytes(bigint);
        }
        _ => return None,
    }
    Some(out)
}

pub(crate) fn typedarray_element_read(
    caller: &mut Caller<'_, RuntimeState>,
    typedarray: i64,
    index: u32,
) -> Option<i64> {
    let entry = typedarray_entry_from_value(caller, typedarray)?;
    typedarray_element_read_entry(caller, &entry, index)
}

pub(crate) fn typedarray_element_read_entry(
    caller: &mut Caller<'_, RuntimeState>,
    entry: &TypedArrayEntry,
    index: u32,
) -> Option<i64> {
    let off = typedarray_element_offset(entry, index)?;
    let mut bytes = [0u8; 8];
    let elem_size = entry.element_size as usize;
    if entry.is_shared {
        let shared = caller.data().shared_state.as_ref()?.clone();
        let sab_table = shared.sab_table.lock().ok()?;
        let buffer = sab_table.get(entry.buffer_handle as usize)?;
        let data = buffer.data.read().ok()?;
        if off + elem_size > data.len() {
            return None;
        }
        bytes[..elem_size].copy_from_slice(&data[off..off + elem_size]);
    } else {
        let ab_table = caller.data().arraybuffer_table.lock().ok()?;
        let buffer = ab_table.get(entry.buffer_handle as usize)?;
        if off + elem_size > buffer.data.len() {
            return None;
        }
        bytes[..elem_size].copy_from_slice(&buffer.data[off..off + elem_size]);
    }
    decode_typedarray_element(caller, &bytes, entry.element_size, entry.element_kind)
}

pub(crate) fn typedarray_element_write(
    caller: &mut Caller<'_, RuntimeState>,
    typedarray: i64,
    index: u32,
    value_raw: i64,
) -> bool {
    let Some(entry) = typedarray_entry_from_value(caller, typedarray) else {
        return false;
    };
    let Some(off) = typedarray_element_offset(&entry, index) else {
        return false;
    };
    let Some(bytes) =
        encode_typedarray_element(caller, entry.element_size, entry.element_kind, value_raw)
    else {
        return false;
    };
    let elem_size = entry.element_size as usize;
    if entry.is_shared {
        let Some(shared) = caller.data().shared_state.as_ref().cloned() else {
            return false;
        };
        let Ok(sab_table) = shared.sab_table.lock() else {
            return false;
        };
        let Some(buffer) = sab_table.get(entry.buffer_handle as usize) else {
            return false;
        };
        let Ok(mut data) = buffer.data.write() else {
            return false;
        };
        if off + elem_size > data.len() {
            return false;
        }
        data[off..off + elem_size].copy_from_slice(&bytes[..elem_size]);
        true
    } else {
        let Ok(mut ab_table) = caller.data().arraybuffer_table.lock() else {
            return false;
        };
        let Some(buffer) = ab_table.get_mut(entry.buffer_handle as usize) else {
            return false;
        };
        if off + elem_size > buffer.data.len() {
            return false;
        }
        buffer.data[off..off + elem_size].copy_from_slice(&bytes[..elem_size]);
        true
    }
}

pub(crate) fn typedarray_construct(
    caller: &mut Caller<'_, RuntimeState>,
    buffer: i64,
    byte_offset: i64,
    length: i64,
    elem_size: u8,
    element_kind: u8,
    target_obj: Option<i64>,
) -> i64 {
    let elem_size_u32 = elem_size as u32;
    let mut initial_values: Option<Vec<i64>> = None;
    let mut backing_is_shared = false;
    let mut buffer_object = None;

    let (buf_handle, offset, len, byte_len) = if value::is_array(buffer) {
        let Some(arr_ptr) = resolve_array_ptr(caller, buffer) else {
            return value::encode_undefined();
        };
        let len = read_array_length(caller, arr_ptr).unwrap_or(0);
        let Some(byte_len) = typedarray_byte_len(caller, len, elem_size_u32) else {
            return value::encode_undefined();
        };
        let mut values = Vec::with_capacity(len as usize);
        for i in 0..len {
            values
                .push(read_array_elem(caller, arr_ptr, i).unwrap_or_else(value::encode_undefined));
        }
        let handle = {
            let mut table = caller
                .data()
                .arraybuffer_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let handle = table.len() as u32;
            table.push(ArrayBufferEntry {
                data: vec![0; byte_len],
            });
            handle
        };
        initial_values = Some(values);
        (handle, 0, len, byte_len)
    } else if let Some(src_entry) = typedarray_entry_from_value(caller, buffer) {
        let len = src_entry.length;
        let Some(byte_len) = typedarray_byte_len(caller, len, elem_size_u32) else {
            return value::encode_undefined();
        };
        let mut values = Vec::with_capacity(len as usize);
        for i in 0..len {
            values.push(
                typedarray_element_read(caller, buffer, i).unwrap_or_else(value::encode_undefined),
            );
        }
        let handle = {
            let mut table = caller
                .data()
                .arraybuffer_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let handle = table.len() as u32;
            table.push(ArrayBufferEntry {
                data: vec![0; byte_len],
            });
            handle
        };
        initial_values = Some(values);
        (handle, 0, len, byte_len)
    } else if value::is_object(buffer) {
        let Some(offset) = typedarray_to_index(
            caller,
            byte_offset,
            "RangeError: Invalid typed array byteOffset",
        ) else {
            return value::encode_undefined();
        };
        let Some(obj_ptr) = resolve_handle(caller, buffer) else {
            return value::encode_undefined();
        };
        let byte_len_val = read_object_property_by_name(caller, obj_ptr, "byteLength");
        let Some(byte_len_val) = byte_len_val else {
            return value::encode_undefined();
        };
        let byte_len = value::decode_f64(byte_len_val) as u32;
        let (buf_handle, is_shared_from_backing) =
            match crate::shared_buffer::resolve_buffer_backing(caller, buffer) {
                Some(crate::shared_buffer::BufferBacking::SharedArrayBuffer { handle, .. }) => {
                    (handle, true)
                }
                Some(crate::shared_buffer::BufferBacking::ArrayBuffer { handle, .. }) => {
                    (handle, false)
                }
                _ => return value::encode_undefined(),
            };
        backing_is_shared = is_shared_from_backing;
        buffer_object = Some(buffer);
        if offset > byte_len || offset % elem_size_u32 != 0 {
            set_typedarray_runtime_error(caller, "RangeError: Invalid typed array byteOffset");
            return value::encode_undefined();
        }
        let remaining = byte_len - offset;
        let len = if value::is_undefined(length) {
            if !remaining.is_multiple_of(elem_size_u32) {
                set_typedarray_runtime_error(caller, "RangeError: Invalid typed array length");
                return value::encode_undefined();
            }
            remaining / elem_size_u32
        } else {
            let Some(len) =
                typedarray_to_index(caller, length, "RangeError: Invalid typed array length")
            else {
                return value::encode_undefined();
            };
            let Some(byte_count) = len.checked_mul(elem_size_u32) else {
                set_typedarray_runtime_error(caller, "RangeError: Invalid typed array length");
                return value::encode_undefined();
            };
            if byte_count > remaining {
                set_typedarray_runtime_error(caller, "RangeError: Invalid typed array length");
                return value::encode_undefined();
            }
            len
        };
        let Some(view_byte_len) = typedarray_byte_len(caller, len, elem_size_u32) else {
            return value::encode_undefined();
        };
        (buf_handle, offset, len, view_byte_len)
    } else {
        let Some(len) =
            typedarray_to_index(caller, buffer, "RangeError: Invalid typed array length")
        else {
            return value::encode_undefined();
        };
        let Some(byte_len) = typedarray_byte_len(caller, len, elem_size_u32) else {
            return value::encode_undefined();
        };
        let handle = {
            let mut table = caller
                .data()
                .arraybuffer_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let handle = table.len() as u32;
            table.push(ArrayBufferEntry {
                data: vec![0; byte_len],
            });
            handle
        };
        (handle, 0, len, byte_len)
    };

    let handle = {
        let mut table = caller
            .data()
            .typedarray_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(TypedArrayEntry {
            buffer_handle: buf_handle,
            buffer_object,
            byte_offset: offset,
            length: len,
            element_size: elem_size,
            element_kind,
            is_shared: backing_is_shared,
        });
        handle
    };

    let obj = if let Some(target) = target_obj.filter(|target| value::is_object(*target)) {
        target
    } else {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &env, 4)
    };
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "__typedarray_handle__",
        value::encode_f64(handle as f64),
    );
    let _ =
        define_host_data_property_from_caller(caller, obj, "length", value::encode_f64(len as f64));
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "byteLength",
        value::encode_f64(byte_len as f64),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "byteOffset",
        value::encode_f64(offset as f64),
    );

    if let Some(values) = initial_values {
        for (i, value) in values.into_iter().enumerate() {
            if !typedarray_element_write(caller, obj, i as u32, value)
                && caller
                    .data()
                    .runtime_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_some()
            {
                return value::encode_undefined();
            }
        }
    }

    obj
}

pub(crate) fn typedarray_same_value_zero(
    caller: &mut Caller<'_, RuntimeState>,
    left: i64,
    right: i64,
) -> bool {
    if value::is_bigint(left) && value::is_bigint(right) {
        let left_handle = value::decode_bigint_handle(left) as usize;
        let right_handle = value::decode_bigint_handle(right) as usize;
        let Ok(table) = caller.data().bigint_table.lock() else {
            return false;
        };
        return match (table.get(left_handle), table.get(right_handle)) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        };
    }
    same_value_zero(caller, left, right)
}
