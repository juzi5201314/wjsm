use super::*;

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

    if value::is_proxy(val) {
        return Ok("Proxy {}".to_string());
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
        return Ok("[object Object]".to_string());
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

pub(crate) fn write_console_value(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
    prefix: Option<&str>,
) {
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

pub(crate) fn read_value_string_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
) -> Option<Vec<u8>> {
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

pub(crate) fn read_i32_global_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
) -> Option<i32> {
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

pub(crate) fn format_number_js(x: f64) -> String {
    if x == 0.0 {
        return "0".to_string();
    }
    let abs = x.abs();
    if abs >= 1e21 || (abs < 1e-6 && abs > 0.0) {
        let s = format!("{:e}", x);
        return normalize_exponent(&s);
    }
    let s = format!("{}", x);
    s
}

pub(crate) fn format_radix(mut value: i64, radix: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }
    let negative = value < 0;
    if negative {
        value = -value;
    }
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut result = Vec::new();
    while value > 0 {
        result.push(digits[value as usize % radix as usize]);
        value /= radix as i64;
    }
    if negative {
        result.push(b'-');
    }
    result.reverse();
    String::from_utf8(result).unwrap_or_else(|_| "0".to_string())
}

pub(crate) fn normalize_exponent(s: &str) -> String {
    if let Some(pos) = s.find('e') {
        let mantissa = &s[..pos];
        let exp_part = &s[pos + 1..];
        let exp_val: i32 = exp_part.parse().unwrap_or(0);
        format!(
            "{}e{}{}",
            mantissa,
            if exp_val >= 0 { "+" } else { "" },
            exp_val
        )
    } else if let Some(pos) = s.find('E') {
        let mantissa = &s[..pos];
        let exp_part = &s[pos + 1..];
        let exp_val: i32 = exp_part.parse().unwrap_or(0);
        format!(
            "{}e{}{}",
            mantissa,
            if exp_val >= 0 { "+" } else { "" },
            exp_val
        )
    } else {
        s.to_string()
    }
}

pub(crate) fn find_memory_c_string_global(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
) -> Option<u32> {
    let mut needle = Vec::with_capacity(name.len() + 1);
    needle.extend_from_slice(name.as_bytes());
    needle.push(0);
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return None;
    };
    memory
        .data(&*caller)
        .windows(needle.len())
        .position(|window| window == needle.as_slice())
        .map(|offset| offset as u32)
}

pub(crate) fn alloc_heap_c_string_global(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
) -> Option<u32> {
    let heap_ptr = caller
        .get_export("__heap_ptr")
        .and_then(|e| e.into_global())?
        .get(&mut *caller)
        .i32()
        .unwrap_or(0) as usize;
    let bytes = name.as_bytes();
    let end = heap_ptr.checked_add(bytes.len() + 1)?;
    let aligned_end = (end + 7) & !7;
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data_mut(&mut *caller);
        if aligned_end > data.len() {
            return None;
        }
        data[heap_ptr..heap_ptr + bytes.len()].copy_from_slice(bytes);
        data[heap_ptr + bytes.len()] = 0;
        data[end..aligned_end].fill(0);
    }
    if let Some(Extern::Global(global)) = caller.get_export("__heap_ptr") {
        let _ = global.set(&mut *caller, Val::I32(aligned_end as i32));
    }
    Some(heap_ptr as u32)
}
