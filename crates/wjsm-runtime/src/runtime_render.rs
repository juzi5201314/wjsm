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
            // C2: 先检查 __error_brand__，仅真实 Error 对象才渲染为 "Name: message"。
            if let Some(brand_val) = read_object_property_by_name(caller, op, "__error_brand__") {
                if value::is_bool(brand_val) && value::decode_bool(brand_val) {
                    if let Some(name_val) = read_object_property_by_name(caller, op, "name") {
                        let name = render_value(caller, name_val).unwrap_or_default();
                        let message = read_object_property_by_name(caller, op, "message")
                            .map(|message_val| {
                                render_value(caller, message_val).unwrap_or_default()
                            })
                            .unwrap_or_default();
                        if message.is_empty() {
                            return Ok(name);
                        }
                        return Ok(format!("{name}: {message}"));
                    }
                }
            }

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
            // TypedArray 渲染
            let ta_handle_val = read_object_property_by_name(caller, op, "__typedarray_handle__");
            if let Some(th) = ta_handle_val {
                let ta_handle = value::decode_f64(th) as usize;
                let (entry, buf_data) = {
                    let ta_table = caller
                        .data()
                        .typedarray_table
                        .lock()
                        .expect("typedarray_table mutex");
                    let entry = ta_table.get(ta_handle).cloned();
                    let buf_data = entry.as_ref().and_then(|e| {
                        let ab_table = caller
                            .data()
                            .arraybuffer_table
                            .lock()
                            .expect("arraybuffer_table mutex");
                        ab_table
                            .get(e.buffer_handle as usize)
                            .map(|b| b.data.clone())
                    });
                    (entry, buf_data)
                };
                if let (Some(entry), Some(buf_data)) = (entry, buf_data) {
                    let mut parts = Vec::new();
                    for i in 0..entry.length {
                        let byte_off = entry.byte_offset as usize
                            + (i as usize) * (entry.element_size as usize);
                        let end = byte_off + entry.element_size as usize;
                        if end <= buf_data.len() {
                            let val = match (entry.element_size, entry.element_kind) {
                                (1, 0) => format!("{}", buf_data[byte_off] as i8),
                                (1, 1) | (1, 2) => format!("{}", buf_data[byte_off]),
                                (2, 0) => format!(
                                    "{}",
                                    i16::from_le_bytes([
                                        buf_data[byte_off],
                                        buf_data[byte_off + 1]
                                    ])
                                ),
                                (2, 1) => format!(
                                    "{}",
                                    u16::from_le_bytes([
                                        buf_data[byte_off],
                                        buf_data[byte_off + 1]
                                    ])
                                ),
                                (4, 0) => format!(
                                    "{}",
                                    i32::from_le_bytes([
                                        buf_data[byte_off],
                                        buf_data[byte_off + 1],
                                        buf_data[byte_off + 2],
                                        buf_data[byte_off + 3]
                                    ])
                                ),
                                (4, 1) => format!(
                                    "{}",
                                    u32::from_le_bytes([
                                        buf_data[byte_off],
                                        buf_data[byte_off + 1],
                                        buf_data[byte_off + 2],
                                        buf_data[byte_off + 3]
                                    ])
                                ),
                                (4, 3) => format!(
                                    "{}",
                                    f32::from_le_bytes([
                                        buf_data[byte_off],
                                        buf_data[byte_off + 1],
                                        buf_data[byte_off + 2],
                                        buf_data[byte_off + 3]
                                    ])
                                ),
                                (8, 3) => format!(
                                    "{}",
                                    f64::from_le_bytes([
                                        buf_data[byte_off],
                                        buf_data[byte_off + 1],
                                        buf_data[byte_off + 2],
                                        buf_data[byte_off + 3],
                                        buf_data[byte_off + 4],
                                        buf_data[byte_off + 5],
                                        buf_data[byte_off + 6],
                                        buf_data[byte_off + 7]
                                    ])
                                ),
                                (8, 4) => {
                                    let v = i64::from_le_bytes([
                                        buf_data[byte_off],
                                        buf_data[byte_off + 1],
                                        buf_data[byte_off + 2],
                                        buf_data[byte_off + 3],
                                        buf_data[byte_off + 4],
                                        buf_data[byte_off + 5],
                                        buf_data[byte_off + 6],
                                        buf_data[byte_off + 7],
                                    ]);
                                    format!("{v}n")
                                }
                                (8, 5) => {
                                    let v = u64::from_le_bytes([
                                        buf_data[byte_off],
                                        buf_data[byte_off + 1],
                                        buf_data[byte_off + 2],
                                        buf_data[byte_off + 3],
                                        buf_data[byte_off + 4],
                                        buf_data[byte_off + 5],
                                        buf_data[byte_off + 6],
                                        buf_data[byte_off + 7],
                                    ]);
                                    format!("{v}n")
                                }
                                _ => "?".to_string(),
                            };
                            parts.push(val);
                        }
                    }
                    return Ok(format!(
                        "TypedArray({}) [{}]",
                        entry.length,
                        parts.join(", ")
                    ));
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

    let n = value::decode_f64(val);
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
/// 从影子栈读取多个值并以空格拼接输出（console varargs 支持）
pub(crate) fn write_console_values(
    caller: &mut Caller<'_, RuntimeState>,
    args_base: i32,
    args_count: i32,
    prefix: Option<&str>,
) {
    let mut rendered = Vec::new();
    for i in 0..args_count as u32 {
        let val = read_shadow_arg(caller, args_base, i);
        rendered.push(render_value(caller, val).unwrap_or_else(|_| "unknown".to_string()));
    }
    let line = rendered.join(" ");
    let mut buffer = caller
        .data()
        .output
        .lock()
        .expect("runtime output buffer mutex should not be poisoned");
    match prefix {
        Some(p) => writeln!(&mut *buffer, "[{p}] {line}"),
        None => writeln!(&mut *buffer, "{line}"),
    }
    .expect("write_console_values should write to the configured output sink");
}

/// JSON 字符串字面量转义（ES §24.5.2 QuoteJSONString）
/// - 补充平面字符直接输出 UTF-8（不使用 surrogate pair，review key fix 1）
/// - 仅转义 " \ 和控制字符，其余 unicode 直接保留（合法 JSON）
fn json_escape_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 2);
    result.push('"');
    for c in s.chars() {
        match c {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{0008}' => result.push_str("\\b"),
            '\u{000C}' => result.push_str("\\f"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            c if (c as u32) < 0x20 => result.push_str(&format!("\\u{:04x}", c as u32)),
            _ => result.push(c),
        }
    }
    result.push('"');
    result
}

/// 构建 space 参数对应的 gap 缩进串（ES §24.5.2 steps 3-6）
/// Number 走 ToIntegerOrInfinity（trunc as i32）；String 按 UTF-16 code unit 截断到 10。
fn build_space_string(caller: &mut Caller<'_, RuntimeState>, space: i64) -> String {
    if value::is_f64(space) {
        let n = value::decode_f64(space);
        let i = n.trunc() as i32;
        if i > 0 {
            let w = i.min(10).max(0) as usize;
            " ".repeat(w)
        } else {
            String::new()
        }
    } else if value::is_string(space) {
        let s = read_runtime_string(caller, space);
        truncate_utf16_prefix(&s, 10)
    } else {
        String::new()
    }
}

/// 从 replacer 数组构建白名单（ES §24.5.2 step 4）
/// 返回 Some(Vec) 表示显式 property list；None 表示未提供数组 replacer。
fn build_replacer_whitelist(
    caller: &mut Caller<'_, RuntimeState>,
    replacer: i64,
) -> Option<Vec<String>> {
    if !value::is_array(replacer) {
        return None;
    }
    let Some(ptr) = resolve_array_ptr(caller, replacer) else {
        return Some(Vec::new());
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let mut list = Vec::new();
    for i in 0..len {
        let Some(elem) = read_array_elem(caller, ptr, i) else {
            continue;
        };
        if value::is_symbol(elem) {
            continue;
        }
        if value::is_string(elem) || value::is_f64(elem) {
            let key = if value::is_string(elem) {
                read_runtime_string(caller, elem)
            } else {
                let f = value::decode_f64(elem);
                if f.is_finite() {
                    if f.fract() == 0.0 && f.abs() <= 9007199254740991.0 {
                        (f as i64).to_string()
                    } else {
                        f.to_string()
                    }
                } else {
                    continue;
                }
            };
            if !list.contains(&key) {
                list.push(key);
            }
        }
    }
    Some(list)
}

/// 获取并调用对象的 toJSON 方法（ES §24.5.2 SerializeJSONProperty 步骤 2）
/// async 版本：使用 call_wasm_callback_async 替代 sync call_wasm_callback
async fn get_to_json_async(caller: &mut Caller<'_, RuntimeState>, key: &str, value: i64) -> i64 {
    if !value::is_object(value) && !value::is_array(value) {
        return value;
    }
    let ptr_opt = if value::is_array(value) {
        resolve_handle_idx(caller, value::decode_array_handle(value) as usize)
    } else {
        resolve_handle(caller, value)
    };
    let Some(ptr) = ptr_opt else {
        return value;
    };
    let to_json =
        read_object_property_by_name(caller, ptr, "toJSON").unwrap_or_else(value::encode_undefined);
    if !is_callable_in_runtime(caller, to_json) {
        return value;
    }
    let key_str = store_runtime_string(caller, key.to_string());
    match call_wasm_callback_async(caller, to_json, value, &[key_str]).await {
        Ok(v) => v,
        Err(_) => value::encode_undefined(),
    }
}
/// 完整的 JSON.stringify（ES §24.5.2），返回 boxed JS 值。
/// async 版本：使用 call_wasm_callback_async 替代 sync call_wasm_callback
pub(crate) async fn runtime_json_stringify_full_async(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
    replacer: i64,
    space: i64,
) -> i64 {
    let gap = build_space_string(caller, space);
    let property_list = build_replacer_whitelist(caller, replacer);
    let replacer_is_fn = is_callable_in_runtime(caller, replacer);
    let replacer_fn = if replacer_is_fn { Some(replacer) } else { None };
    let mut stack = Vec::new();
    let json = serialize_json_property_async(
        caller,
        "",
        val,
        replacer_is_fn,
        replacer_fn,
        property_list.as_deref(),
        &mut stack,
        &gap,
        "",
    )
    .await;
    if json == "undefined" {
        value::encode_undefined()
    } else {
        store_runtime_string(caller, json)
    }
}

/// 序列化 JSON 属性（核心递归 impl，含 cycle、toJSON、replacer、pretty-print）
/// async 版本：使用 call_wasm_callback_async 替代 sync call_wasm_callback
async fn serialize_json_property_async(
    caller: &mut Caller<'_, RuntimeState>,
    key: &str,
    val: i64,
    replacer_is_fn: bool,
    replacer_fn: Option<i64>,
    property_list: Option<&[String]>,
    stack: &mut Vec<i64>,
    gap: &str,
    current_indent: &str,
) -> String {
    let mut value = get_to_json_async(caller, key, val).await;
    let mut replacer_returned_undefined = false;
    if let Some(rf) = replacer_fn.filter(|_| replacer_is_fn) {
        let key_str = store_runtime_string(caller, key.to_string());
        match call_wasm_callback_async(caller, rf, value, &[key_str, value]).await {
            Ok(new_val) => {
                replacer_returned_undefined = value::is_undefined(new_val);
                value = new_val;
            }
            Err(_) => {
                replacer_returned_undefined = true;
                value = value::encode_undefined();
            }
        }
    }
    if value::is_f64(value) {
        let f = value::decode_f64(value);
        return if !f.is_finite() {
            "null".to_string()
        } else if f == 0.0 {
            "0".to_string()
        } else {
            f.to_string()
        };
    }
    if value::is_undefined(value) {
        if replacer_returned_undefined || key.is_empty() {
            return "undefined".to_string();
        }
        return "null".to_string();
    }
    if value::is_callable(value) || value::is_symbol(value) {
        return "undefined".to_string();
    }
    if value::is_bigint(value) {
        *caller
            .data()
            .runtime_error
            .lock()
            .expect("runtime error mutex") =
            Some("TypeError: Do not know how to serialize a BigInt".to_string());
        return "null".to_string();
    }
    if value::is_string(value) {
        let s = read_runtime_string(caller, value);
        return json_escape_string(&s);
    }
    if value::is_bool(value) {
        return value::decode_bool(value).to_string();
    }
    if value::is_null(value) {
        return "null".to_string();
    }

    let next_indent = if gap.is_empty() {
        String::new()
    } else {
        format!("{}{}", current_indent, gap)
    };

    if value::is_array(value) {
        if stack.contains(&value) {
            set_runtime_error(
                caller.data(),
                "TypeError: Converting circular structure to JSON".to_string(),
            );
            return "null".to_string();
        }
        stack.push(value);
        let handle_idx = value::decode_array_handle(value) as usize;
        let ptr = match resolve_handle_idx(caller, handle_idx) {
            Some(ptr) => ptr,
            None => {
                stack.pop();
                return "null".to_string();
            }
        };
        let len = read_array_length(caller, ptr).unwrap_or(0);
        let mut parts = Vec::with_capacity(len as usize);
        for i in 0..len {
            let elem = read_array_elem(caller, ptr, i).unwrap_or_else(value::encode_undefined);
            let s = Box::pin(serialize_json_property_async(
                caller,
                &i.to_string(),
                elem,
                replacer_is_fn,
                replacer_fn,
                property_list,
                stack,
                gap,
                &next_indent,
            ))
            .await;
            parts.push(if s == "undefined" {
                "null".to_string()
            } else {
                s
            });
        }
        stack.pop();
        return if parts.is_empty() {
            "[]".to_string()
        } else if gap.is_empty() {
            format!("[{}]", parts.join(","))
        } else {
            let inner = parts.join(&format!(",\n{}", next_indent));
            format!("[\n{}{}\n{}]", next_indent, inner, current_indent)
        };
    }

    if value::is_object(value) {
        if stack.contains(&value) {
            set_runtime_error(
                caller.data(),
                "TypeError: Converting circular structure to JSON".to_string(),
            );
            return "null".to_string();
        }
        stack.push(value);
        let ptr = match resolve_handle(caller, value) {
            Some(ptr) => ptr,
            None => {
                stack.pop();
                return "null".to_string();
            }
        };
        let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
            stack.pop();
            return "null".to_string();
        };
        {
            let data = memory.data(&*caller);
            if ptr + 16 > data.len() {
                stack.pop();
                return "null".to_string();
            }
        }

        let mut pairs = Vec::new();
        if let Some(property_list) = property_list {
            for name in property_list {
                if let Some(prop_val) = read_object_property_by_name(caller, ptr, name) {
                    if value::is_undefined(prop_val) {
                        continue;
                    }
                    let s = Box::pin(serialize_json_property_async(
                        caller,
                        name,
                        prop_val,
                        replacer_is_fn,
                        replacer_fn,
                        Some(property_list),
                        stack,
                        gap,
                        &next_indent,
                    ))
                    .await;
                    if s != "undefined" {
                        let colon = if gap.is_empty() { ":" } else { ": " };
                        pairs.push(format!("{}{}{}", json_escape_string(name), colon, s));
                    }
                }
            }
        } else {
            let slots: Vec<(u32, i64)> = {
                let data = memory.data(&*caller);
                let num_props = u32::from_le_bytes([
                    data[ptr + 12],
                    data[ptr + 13],
                    data[ptr + 14],
                    data[ptr + 15],
                ]) as usize;
                let mut slots = Vec::with_capacity(num_props);
                for i in 0..num_props {
                    let slot_off = ptr + 16 + i * 32;
                    if slot_off + 32 > data.len() {
                        continue;
                    }
                    let flags = i32::from_le_bytes([
                        data[slot_off + 4],
                        data[slot_off + 5],
                        data[slot_off + 6],
                        data[slot_off + 7],
                    ]);
                    if (flags & 2) == 0 {
                        continue;
                    }
                    let name_id = u32::from_le_bytes([
                        data[slot_off],
                        data[slot_off + 1],
                        data[slot_off + 2],
                        data[slot_off + 3],
                    ]);
                    let prop_val =
                        i64::from_le_bytes(data[slot_off + 8..slot_off + 16].try_into().unwrap());
                    if value::is_undefined(prop_val) {
                        continue;
                    }
                    slots.push((name_id, prop_val));
                }
                slots
            };
            for (name_id, prop_val) in slots {
                let name_bytes = read_string_bytes(caller, name_id);
                let name = String::from_utf8_lossy(&name_bytes).to_string();
                let s = Box::pin(serialize_json_property_async(
                    caller,
                    &name,
                    prop_val,
                    replacer_is_fn,
                    replacer_fn,
                    None,
                    stack,
                    gap,
                    &next_indent,
                ))
                .await;
                if s != "undefined" {
                    let colon = if gap.is_empty() { ":" } else { ": " };
                    pairs.push(format!("{}{}{}", json_escape_string(&name), colon, s));
                }
            }
        }

        stack.pop();
        return if pairs.is_empty() {
            "{}".to_string()
        } else if gap.is_empty() {
            format!("{{{}}}", pairs.join(","))
        } else {
            let inner = pairs.join(&format!(",\n{}", next_indent));
            format!("{{\n{}{}\n{}}}", next_indent, inner, current_indent)
        };
    }

    "null".to_string()
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

pub(crate) fn read_string_bytes_mem<C: AsContext>(ctx: &C, memory: &Memory, ptr: u32) -> Vec<u8> {
    let data = memory.data(ctx);
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

pub(crate) fn read_string_bytes(caller: &mut Caller<'_, RuntimeState>, ptr: u32) -> Vec<u8> {
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Vec::new();
    };
    read_string_bytes_mem(caller, &memory, ptr)
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

pub(crate) fn format_number_to_fixed_js(x: f64, digits: i32) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    format!("{:.1$}", x, digits as usize)
}

pub(crate) fn format_number_to_exponential_js(x: f64, digits: Option<i32>) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if x == 0.0 {
        if let Some(digits) = digits
            && digits > 0
        {
            return format!("0.{}e+0", "0".repeat(digits as usize));
        }
        return "0e+0".to_string();
    }
    let s = if let Some(digits) = digits {
        format!("{:.1$e}", x, digits as usize)
    } else {
        format!("{:e}", x)
    };
    normalize_exponent(&s)
}

pub(crate) fn format_number_to_precision_js(x: f64, precision: Option<i32>) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let Some(precision) = precision else {
        return format_number_js(x);
    };
    if x == 0.0 {
        if precision == 1 {
            return "0".to_string();
        }
        return format!("0.{}", "0".repeat((precision - 1) as usize));
    }
    let exponent = x.abs().log10().floor() as i32;
    if exponent >= precision || exponent < -6 {
        let s = format!("{:.1$e}", x, (precision - 1) as usize);
        return normalize_exponent(&s);
    }
    let fraction_digits = (precision - exponent - 1).max(0) as usize;
    format!("{:.1$}", x, fraction_digits)
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
