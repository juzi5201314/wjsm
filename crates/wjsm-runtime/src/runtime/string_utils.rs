use anyhow::Result;
use wasmtime::{Caller, Extern};

use crate::types::{RuntimeState, EvalVarMapEntry};
use wjsm_ir::{constants, value};

pub(crate) fn read_string(caller: &mut Caller<'_, RuntimeState>, ptr: u32) -> Result<String> {
    let data = read_string_bytes(caller, ptr);
    Ok(std::str::from_utf8(&data)?.to_owned())
}

pub(crate) fn read_runtime_string(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    if value::is_runtime_string_handle(val) {
        let handle = value::decode_runtime_string_handle(val) as usize;
        let strings = caller
            .data()
            .runtime_strings
            .lock()
            .expect("runtime strings mutex");
        strings.get(handle).cloned().unwrap_or_default()
    } else if value::is_string(val) {
        let ptr = value::decode_string_ptr(val);
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return String::new();
        };
        let data = memory.data(caller);
        let start = ptr as usize;
        if start >= data.len() {
            return String::new();
        }
        let end = data[start..]
            .iter()
            .position(|byte| *byte == 0)
            .map_or(data.len(), |offset| start + offset);
        std::str::from_utf8(&data[start..end])
            .unwrap_or_default()
            .to_owned()
    } else {
        String::new()
    }
}

pub(crate) fn read_string_bytes(caller: &mut Caller<'_, RuntimeState>, ptr: u32) -> Vec<u8> {
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Vec::new();
    };

    let data = memory.data(caller);
    let start = ptr as usize;
    if start >= data.len() {
        return Vec::new();
    }

    let end = data[start..]
        .iter()
        .position(|byte| *byte == 0)
        .map_or(data.len(), |offset| start + offset);

    data[start..end].to_vec()
}

pub(crate) fn read_value_string_bytes(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Option<Vec<u8>> {
    if !value::is_string(val) {
        return None;
    }

    if value::is_runtime_string_handle(val) {
        let handle = value::decode_runtime_string_handle(val) as usize;
        let strings = caller
            .data()
            .runtime_strings
            .lock()
            .expect("runtime strings mutex");
        return strings.get(handle).map(|string| string.as_bytes().to_vec());
    }

    Some(read_string_bytes(caller, value::decode_string_ptr(val)))
}

pub(crate) fn read_i32_global_from_caller(caller: &mut Caller<'_, RuntimeState>, name: &str) -> Option<i32> {
    caller
        .get_export(name)
        .and_then(Extern::into_global)
        .and_then(|global| global.get(&mut *caller).i32())
}

pub(crate) fn read_u32_le(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub(crate) fn read_utf8_slice(data: &[u8], ptr: u32, len: u32) -> Option<String> {
    let start = ptr as usize;
    let end = start.checked_add(len as usize)?;
    let bytes = data.get(start..end)?;
    std::str::from_utf8(bytes).ok().map(ToOwned::to_owned)
}

pub(crate) fn read_eval_var_map(caller: &mut Caller<'_, RuntimeState>) -> Vec<EvalVarMapEntry> {
    const RECORD_SIZE: usize = 20;

    let ptr = read_i32_global_from_caller(caller, "__eval_var_map_ptr").unwrap_or(0);
    let count = read_i32_global_from_caller(caller, "__eval_var_map_count").unwrap_or(0);
    if ptr <= 0 || count <= 0 {
        return Vec::new();
    }

    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Vec::new();
    };
    let data = memory.data(&*caller);
    let mut entries = Vec::with_capacity(count as usize);

    for index in 0..count as usize {
        let Some(record_offset) = (ptr as usize).checked_add(index * RECORD_SIZE) else {
            break;
        };
        let Some(function_ptr) = read_u32_le(data, record_offset) else {
            break;
        };
        let Some(function_len) = read_u32_le(data, record_offset + 4) else {
            break;
        };
        let Some(var_ptr) = read_u32_le(data, record_offset + 8) else {
            break;
        };
        let Some(var_len) = read_u32_le(data, record_offset + 12) else {
            break;
        };
        let Some(offset) = read_u32_le(data, record_offset + 16) else {
            break;
        };
        let Some(function_name) = read_utf8_slice(data, function_ptr, function_len) else {
            continue;
        };
        let Some(var_name) = read_utf8_slice(data, var_ptr, var_len) else {
            continue;
        };
        entries.push(EvalVarMapEntry {
            function_name,
            var_name,
            offset,
        });
    }

    entries
}

pub(crate) fn store_runtime_string(caller: &Caller<'_, RuntimeState>, string: String) -> i64 {
    let mut strings = caller
        .data()
        .runtime_strings
        .lock()
        .expect("runtime strings mutex");
    let handle = strings.len() as u32;
    strings.push(string);
    value::encode_runtime_string_handle(handle)
}

pub(crate) fn store_runtime_string_in_state(state: &RuntimeState, string: String) -> i64 {
    let mut strings = state.runtime_strings.lock().expect("runtime strings mutex");
    let handle = strings.len() as u32;
    strings.push(string);
    value::encode_runtime_string_handle(handle)
}

