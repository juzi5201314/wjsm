use anyhow::Result;
use std::io::Write;
use wasmtime::Caller;

use crate::types::RuntimeState;
use crate::runtime::string_utils::{read_string, read_value_string_bytes, store_runtime_string};
use crate::runtime::array_ops::{read_array_length, read_array_elem};
use crate::runtime::memory::{resolve_handle_idx, resolve_array_ptr};
use crate::runtime::object_ops::read_object_property_by_name;
use wjsm_ir::value;

pub(crate) fn render_value(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Result<String> {
    if value::is_string(val) {
        if value::is_runtime_string_handle(val) {
            let handle = value::decode_runtime_string_handle(val) as usize;
            let strings = caller
                .data()
                .runtime_strings
                .lock()
                .expect("runtime strings mutex");
            if let Some(value) = strings.get(handle) {
                return Ok(value.clone());
            }
            return Ok(String::new());
        }

        return read_string(caller, value::decode_string_ptr(val));
    }

    if value::is_undefined(val) {
        return Ok("undefined".to_string());
    }

    if value::is_null(val) {
        return Ok("null".to_string());
    }

    if value::is_bool(val) {
        return Ok(if value::decode_bool(val) {
            "true".to_string()
        } else {
            "false".to_string()
        });
    }

    if value::is_iterator(val) {
        let handle = value::decode_handle(val);
        return Ok(format!("[iterator:{handle}]"));
    }

    if value::is_enumerator(val) {
        let handle = value::decode_handle(val);
        return Ok(format!("[enumerator:{handle}]"));
    }

    if value::is_exception(val) {
        let handle = value::decode_handle(val);
        return Ok(format!("[exception:{handle}]"));
    }

    if value::is_array(val) {
        let ptr = resolve_array_ptr(caller, val);
        if let Some(ptr) = ptr {
            let len = read_array_length(caller, ptr).unwrap_or(0);
            let mut parts = Vec::with_capacity(len as usize);
            for i in 0..len {
                if let Some(elem) = read_array_elem(caller, ptr, i) {
                    parts.push(render_value(caller, elem).unwrap_or_else(|_| "?".to_string()));
                } else {
                    parts.push("?".to_string());
                }
            }
            return Ok(format!("[{}]", parts.join(", ")));
        }
        return Ok("[array]".to_string());
    }

    if value::is_object(val) {
        let ptr = value::decode_object_handle(val);
        let obj_ptr = resolve_handle_idx(caller, ptr as usize);
        if let Some(op) = obj_ptr {
            let map_handle = read_object_property_by_name(caller, op, "__map_handle__");
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let keys_values = {
                    let table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        let entry = &table[handle];
                        Some((entry.keys.clone(), entry.values.clone()))
                    } else {
                        None
                    }
                };
                if let Some((keys, values)) = keys_values {
                    let mut parts = Vec::new();
                    for i in 0..keys.len() {
                        let k = render_value(caller, keys[i]).unwrap_or_else(|_| "?".to_string());
                        let v = render_value(caller, values[i]).unwrap_or_else(|_| "?".to_string());
                        parts.push(format!("{k} => {v}"));
                    }
                    return Ok(format!("Map {{{}}}", parts.join(", ")));
                }
            }
            let set_handle = read_object_property_by_name(caller, op, "__set_handle__");
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let vals = {
                    let table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        Some(table[handle].values.clone())
                    } else {
                        None
                    }
                };
                if let Some(values) = vals {
                    let mut parts = Vec::new();
                    for v in &values {
                        parts.push(render_value(caller, *v).unwrap_or_else(|_| "?".to_string()));
                    }
                    return Ok(format!("Set {{{}}}", parts.join(", ")));
                }
            }
        }
        return Ok(format!("[object Object:{}]", ptr));
    }

    // TODO: 函数的 toString() 应显示函数名（如 "function foo() { [native code] }"），
    // 但当前 RuntimeState 未存储函数名信息，需要后续添加 function_names 侧表。
    if value::is_function(val) {
        return Ok("function() { [native code] }".to_string());
    }

    if value::is_closure(val) {
        return Ok("function() { [native code] }".to_string());
    }

    if value::is_bigint(val) {
        let handle = value::decode_bigint_handle(val) as usize;
        let table = caller
            .data()
            .bigint_table
            .lock()
            .expect("bigint_table mutex");
        if let Some(bigint) = table.get(handle) {
            return Ok(format!("{bigint}n"));
        }
        return Ok("0n".to_string());
    }

    if value::is_symbol(val) {
        let handle = value::decode_symbol_handle(val) as usize;
        let table = caller
            .data()
            .symbol_table
            .lock()
            .expect("symbol_table mutex");
        if let Some(entry) = table.get(handle) {
            if let Some(ref desc) = entry.description {
                // Escape the description for display
                return Ok(format!("Symbol({})", desc));
            }
            return Ok("Symbol()".to_string());
        }
        return Ok("Symbol()".to_string());
    }

    if value::is_regexp(val) {
        let handle = value::decode_regexp_handle(val) as usize;
        let table = caller.data().regex_table.lock().expect("regex_table mutex");
        if let Some(entry) = table.get(handle) {
            return Ok(format!(
                "/{}/{}",
                entry.pattern.replace('/', "\\/"),
                entry.flags
            ));
        }
        return Ok("/(?:)/".to_string()); // empty regex fallback
    }

    let n = f64::from_bits(val as u64);
    if n.is_infinite() {
        return Ok(if n.is_sign_positive() {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        });
    }
    Ok(n.to_string())
}

pub(crate) fn write_console_value(caller: &mut Caller<'_, RuntimeState>, val: i64, prefix: Option<&str>) {
    let rendered = render_value(caller, val).unwrap_or_else(|_| "unknown".to_string());
    let mut buffer = caller
        .data()
        .output
        .lock()
        .expect("runtime output buffer mutex should not be poisoned");
    match prefix {
        Some(p) => writeln!(&mut *buffer, "[{p}] {rendered}"),
        None => writeln!(&mut *buffer, "{rendered}"),
    }
    .expect("write_console_value should write to the configured output sink");
}

/// 简单的 URL 编码解码（支持 %XX 和 + → 空格）
pub(crate) fn urlencoding_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        match c {
            '+' => result.push(' '),
            '%' => {
                let hex: String = chars.by_ref().take(2).collect();
                if hex.len() == 2 {
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        result.push(byte as char);
                    } else {
                        result.push('%');
                        result.push_str(&hex);
                    }
                } else {
                    result.push('%');
                    result.push_str(&hex);
                }
            }
            other => result.push(other),
        }
    }
    result
}

/// 简单的 JSON.stringify 实现（简化版，仅支持基本类型）
pub(crate) fn runtime_json_stringify(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    if value::is_f64(val) {
        let f = f64::from_bits(val as u64);
        if f.is_finite() {
            f.to_string()
        } else {
            "null".to_string()
        }
    } else if value::is_string(val) {
        let s = if value::is_runtime_string_handle(val) {
            let handle = value::decode_runtime_string_handle(val) as usize;
            caller
                .data()
                .runtime_strings
                .lock()
                .expect("runtime strings mutex")
                .get(handle)
                .cloned()
                .unwrap_or_default()
        } else {
            read_string(caller, value::decode_string_ptr(val)).unwrap_or_default()
        };
        format!("\"{}\"", s.escape_default())
    } else if value::is_bool(val) {
        value::decode_bool(val).to_string()
    } else if value::is_null(val) {
        "null".to_string()
    } else if value::is_undefined(val) || value::is_callable(val) {
        "undefined".to_string()
    } else if value::is_bigint(val) {
        // JSON.stringify on BigInt throws TypeError
        *caller
            .data()
            .runtime_error
            .lock()
            .expect("runtime error mutex") =
            Some("TypeError: Do not know how to serialize a BigInt".to_string());
        "null".to_string()
    } else if value::is_symbol(val) {
        // JSON.stringify 返回 undefined for Symbol values
        "undefined".to_string()
    } else if value::is_object(val) {
        "[object Object]".to_string()
    } else {
        "null".to_string()
    }
}
