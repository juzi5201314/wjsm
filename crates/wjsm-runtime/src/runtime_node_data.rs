use wasmtime::Caller;

use crate::runtime_buffer::{arraybuffer_visible_bytes, visible_bytes};
use crate::runtime_encoding::js_string_lossy;
use crate::runtime_values::{read_array_elem, read_array_length, read_object_property_by_name, resolve_handle};
use crate::{RuntimeState, make_type_error_exception, value};

pub(crate) fn bytes_from_value(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
    what: &str,
) -> Result<Vec<u8>, i64> {
    if value::is_undefined(value_raw) || value::is_null(value_raw) {
        return Ok(Vec::new());
    }
    if value::is_string(value_raw) {
        return Ok(js_string_lossy(caller, value_raw).into_bytes());
    }
    if let Some(bytes) = visible_bytes(caller, value_raw) {
        return Ok(bytes);
    }
    if let Some(bytes) = arraybuffer_visible_bytes(caller, value_raw) {
        return Ok(bytes);
    }
    Err(make_type_error_exception(
        caller,
        &format!("{what} must be a string, Buffer, TypedArray, DataView, or ArrayBuffer"),
    ))
}

pub(crate) fn object_string_property(
    caller: &mut Caller<'_, RuntimeState>,
    object: i64,
    name: &str,
) -> Option<String> {
    let ptr = resolve_handle(caller, object)?;
    let raw = read_object_property_by_name(caller, ptr, name)?;
    if value::is_string(raw) {
        Some(js_string_lossy(caller, raw))
    } else if value::is_undefined(raw) || value::is_null(raw) {
        None
    } else {
        Some(crate::render_value(caller, raw).unwrap_or_default())
    }
}

pub(crate) fn object_number_property(
    caller: &mut Caller<'_, RuntimeState>,
    object: i64,
    name: &str,
) -> Option<f64> {
    let ptr = resolve_handle(caller, object)?;
    let raw = read_object_property_by_name(caller, ptr, name)?;
    if value::is_f64(raw) {
        Some(value::decode_f64(raw))
    } else {
        None
    }
}

pub(crate) fn object_bool_property(
    caller: &mut Caller<'_, RuntimeState>,
    object: i64,
    name: &str,
) -> Option<bool> {
    let ptr = resolve_handle(caller, object)?;
    let raw = read_object_property_by_name(caller, ptr, name)?;
    if value::is_bool(raw) {
        Some(value::decode_bool(raw))
    } else {
        None
    }
}

pub(crate) fn string_array_from_value(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
) -> Result<Vec<String>, i64> {
    if value::is_undefined(value_raw) || value::is_null(value_raw) {
        return Ok(Vec::new());
    }
    if !value::is_array(value_raw) {
        return Err(make_type_error_exception(caller, "expected an array of strings"));
    }
    let Some(ptr) = resolve_handle(caller, value_raw) else {
        return Ok(Vec::new());
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let mut out = Vec::with_capacity(len as usize);
    for index in 0..len {
        let Some(item) = read_array_elem(caller, ptr, index) else {
            out.push(String::new());
            continue;
        };
        if value::is_string(item) {
            out.push(js_string_lossy(caller, item));
        } else {
            out.push(crate::render_value(caller, item).unwrap_or_default());
        }
    }
    Ok(out)
}
