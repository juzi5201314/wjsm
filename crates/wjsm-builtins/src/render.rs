//! console 值渲染与 JSON.stringify（ES §24.5.2）后端无关实现。
//!
//! 算法逻辑通过 `<E: ExecContext>` 泛型单态化，零 vtable 开销。
//! host-wasm 的 `runtime_render.rs` 仅保留字符串/线性内存原语与薄委托层。

use crate::number_format::format_number_js;
use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

/// 将值渲染为人类可读字符串（console.log 规则）。
#[inline]
pub fn render_value_impl<E: ExecContext>(ctx: &mut E, val: Value) -> String {
    if value::is_string(val) {
        return ctx.get_runtime_string(val).to_utf8_lossy();
    }

    if value::is_undefined(val) {
        return "undefined".to_string();
    }

    if value::is_null(val) {
        return "null".to_string();
    }

    if value::is_bool(val) {
        return if value::decode_bool(val) {
            "true".to_string()
        } else {
            "false".to_string()
        };
    }

    if value::is_iterator(val) {
        let handle = value::decode_handle(val);
        return format!("[iterator:{handle}]");
    }

    if value::is_enumerator(val) {
        let handle = value::decode_handle(val);
        return format!("[enumerator:{handle}]");
    }

    if value::is_exception(val) {
        let handle = value::decode_handle(val);
        return format!("[exception:{handle}]");
    }

    if value::is_array(val) {
        if let Some(len) = ctx.array_read_length(val) {
            let mut parts = Vec::with_capacity(len as usize);
            for i in 0..len {
                if let Some(elem) = ctx.array_elem_at(val, i) {
                    parts.push(render_value_impl(ctx, elem));
                } else {
                    parts.push("?".to_string());
                }
            }
            return format!("[{}]", parts.join(", "));
        }
        return "[array]".to_string();
    }

    if value::is_proxy(val) {
        return "Proxy {}".to_string();
    }

    if value::is_object(val) {
        // C2: 先检查 __error_brand__，仅真实 Error 对象才渲染为 "Name: message"。
        if let Some(brand_val) = ctx.read_property_for_render(val, "__error_brand__")
            && value::is_bool(brand_val)
            && value::decode_bool(brand_val)
            && let Some(name_val) = ctx.read_property_for_render(val, "name")
        {
            let name = render_value_impl(ctx, name_val);
            let message = ctx
                .read_property_for_render(val, "message")
                .map(|message_val| render_value_impl(ctx, message_val))
                .unwrap_or_default();
            if message.is_empty() {
                return name;
            }
            return format!("{name}: {message}");
        }

        if let Some(mh) = ctx.read_property_for_render(val, "__map_handle__") {
            let handle = value::decode_f64(mh) as u32;
            let entries = ctx.map_set_entries_snapshot(handle, false);
            let mut parts = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let k = render_value_impl(ctx, k);
                let v = render_value_impl(ctx, v);
                parts.push(format!("{k} => {v}"));
            }
            return format!("Map {{{}}}", parts.join(", "));
        }

        if let Some(sh) = ctx.read_property_for_render(val, "__set_handle__") {
            let handle = value::decode_f64(sh) as u32;
            let entries = ctx.map_set_entries_snapshot(handle, true);
            let mut parts = Vec::with_capacity(entries.len());
            for (v, _) in entries {
                parts.push(render_value_impl(ctx, v));
            }
            return format!("Set {{{}}}", parts.join(", "));
        }

        // TypedArray 渲染
        if ctx
            .read_property_for_render(val, "__typedarray_handle__")
            .is_some()
            && let Some(view) = ctx.typedarray_resolve(val)
        {
            let byte_len = if view.is_shared {
                ctx.shared_arraybuffer_byte_length(view.buffer_handle)
            } else {
                ctx.arraybuffer_byte_length(view.buffer_handle)
            };
            if let Some(byte_len) = byte_len
                && let Some(buf_data) =
                    ctx.buffer_read_bytes(view.buffer_handle, view.is_shared, 0, byte_len as usize)
            {
                let mut parts = Vec::new();
                for i in 0..view.length {
                    let byte_off =
                        view.byte_offset as usize + (i as usize) * (view.element_size as usize);
                    let end = byte_off + view.element_size as usize;
                    if end <= buf_data.len()
                        && let Some(rendered) = render_typedarray_elem(
                            &buf_data[byte_off..end],
                            view.element_size,
                            view.element_kind,
                        )
                    {
                        parts.push(rendered);
                    }
                }
                return format!("TypedArray({}) [{}]", view.length, parts.join(", "));
            }
        }
        return "[object Object]".to_string();
    }

    // 函数值使用统一 native-code 表示；`name` 属性由函数属性表独立提供。
    if value::is_function(val) {
        return "function() { [native code] }".to_string();
    }

    if value::is_closure(val) {
        return "function() { [native code] }".to_string();
    }

    if value::is_bigint(val) {
        if let Some(bigint) = ctx.read_bigint(val) {
            return format!("{bigint}");
        }
        return "0".to_string();
    }

    if value::is_symbol(val) {
        if let Some((description, _)) = ctx.symbol_entry(val) {
            if let Some(desc) = description {
                return format!("Symbol({desc})");
            }
            return "Symbol()".to_string();
        }
        return "Symbol()".to_string();
    }

    if value::is_regexp(val) {
        if let Some((pattern, flags)) = ctx.regexp_pattern_flags(val) {
            return format!("/{}/{}", pattern.replace('/', "\\/"), flags);
        }
        return "/(?:)/".to_string(); // empty regex fallback
    }

    let n = value::decode_f64(val);
    if n.is_infinite() {
        return if n.is_sign_positive() {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    n.to_string()
}

/// 按元素字节解码 TypedArray 元素为显示串（与线性内存布局一致）。
fn render_typedarray_elem(bytes: &[u8], element_size: u8, element_kind: u8) -> Option<String> {
    let val = match (element_size, element_kind) {
        (1, 0) => format!("{}", bytes[0] as i8),
        (1, 1) | (1, 2) => format!("{}", bytes[0]),
        (2, 0) => format!("{}", i16::from_le_bytes([bytes[0], bytes[1]])),
        (2, 1) => format!("{}", u16::from_le_bytes([bytes[0], bytes[1]])),
        (4, 0) => format!(
            "{}",
            i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        ),
        (4, 1) => format!(
            "{}",
            u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        ),
        (4, 3) => format!(
            "{}",
            f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        ),
        (8, 3) => format!(
            "{}",
            f64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]
            ])
        ),
        (8, 4) => {
            let v = i64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]);
            format!("{v}n")
        }
        (8, 5) => {
            let v = u64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]);
            format!("{v}n")
        }
        _ => "?".to_string(),
    };
    Some(val)
}

/// console 单值输出（渲染 + 换行写入 stdout 缓冲）。
pub fn write_console_value_impl<E: ExecContext>(ctx: &mut E, val: Value, prefix: Option<&str>) {
    let rendered = render_value_impl(ctx, val);
    let line = match prefix {
        Some(p) => format!("[{p}] {rendered}\n"),
        None => format!("{rendered}\n"),
    };
    ctx.write_output(line.as_bytes());
}
/// 从影子栈读取多个值并以空格拼接输出（console varargs 支持）。
#[inline]
pub fn write_console_values_impl<E: ExecContext>(
    ctx: &mut E,
    args_base: i32,
    args_count: i32,
    prefix: Option<&str>,
) {
    let mut rendered = Vec::new();
    for i in 0..args_count as u32 {
        let val = ctx.read_shadow_arg(args_base, i);
        rendered.push(render_value_impl(ctx, val));
    }
    let joined = rendered.join(" ");
    let line = match prefix {
        Some(p) => format!("[{p}] {joined}\n"),
        None => format!("{joined}\n"),
    };
    ctx.write_output(line.as_bytes());
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
fn build_space_string<E: ExecContext>(ctx: &mut E, space: Value) -> String {
    if value::is_f64(space) {
        let n = value::decode_f64(space);
        let i = n.trunc() as i32;
        if i > 0 {
            let w = i.clamp(0, 10) as usize;
            " ".repeat(w)
        } else {
            String::new()
        }
    } else if value::is_string(space) {
        let s = ctx.get_runtime_string(space);
        s.slice_units(0..s.utf16_len().min(10)).to_utf8_lossy()
    } else {
        String::new()
    }
}

/// 从 replacer 数组构建白名单（ES §24.5.2 step 4）
/// 返回 Some(Vec) 表示显式 property list；None 表示未提供数组 replacer。
fn build_replacer_whitelist<E: ExecContext>(ctx: &mut E, replacer: Value) -> Option<Vec<String>> {
    if !value::is_array(replacer) {
        return None;
    }
    let Some(len) = ctx.array_read_length(replacer) else {
        return Some(Vec::new());
    };
    let mut list = Vec::new();
    for i in 0..len {
        let Some(elem) = ctx.array_elem_at(replacer, i) else {
            continue;
        };
        if value::is_symbol(elem) {
            continue;
        }
        if value::is_string(elem) || value::is_f64(elem) {
            let key = if value::is_string(elem) {
                ctx.get_runtime_string(elem).to_utf8_lossy()
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

/// 获取并调用对象的 toJSON 方法（ES §24.5.2 SerializeJSONProperty 步骤 2）。
async fn get_to_json<E: ExecContext>(ctx: &mut E, key: &str, value: Value) -> Value {
    if !value::is_object(value) && !value::is_array(value) {
        return value;
    }
    let Some(to_json) = ctx.read_property_for_render(value, "toJSON") else {
        return value;
    };
    if !ctx.is_callable(to_json) {
        return value;
    }
    let key_str = ctx.store_string(key);
    let args = [key_str];
    ctx.call_js_async(to_json, value, &args)
        .await
        .unwrap_or_else(|_| value::encode_undefined())
}

/// ES §24.5.2 step 10.a：顶层 replacer 调用前构造 `{"": value}` 包装对象。
fn make_stringify_root_wrapper<E: ExecContext>(ctx: &mut E, val: Value) -> Value {
    let root = ctx.alloc_object(1);
    ctx.define_data_property(root, "", val);
    root
}

/// 负零规范化（JSON.stringify 输出 "0" 而非 "-0"）。
fn normalize_negative_zero(x: f64) -> f64 {
    if x == 0.0 && x.is_sign_negative() {
        0.0
    } else {
        x
    }
}

/// 完整的 JSON.stringify（ES §24.5.2），返回 boxed JS 值。
pub async fn json_stringify_full_impl<E: ExecContext>(
    ctx: &mut E,
    val: Value,
    replacer: Value,
    space: Value,
) -> Value {
    let gap = build_space_string(ctx, space);
    let property_list = build_replacer_whitelist(ctx, replacer);
    let replacer_is_fn = ctx.is_callable(replacer);
    let replacer_fn = if replacer_is_fn { Some(replacer) } else { None };
    let holder = if replacer_is_fn {
        make_stringify_root_wrapper(ctx, val)
    } else {
        value::encode_undefined()
    };
    let mut stack = Vec::new();
    match serialize_json_property(
        ctx,
        "",
        val,
        holder,
        replacer_is_fn,
        replacer_fn,
        property_list.as_deref(),
        &mut stack,
        &gap,
        "",
    )
    .await
    {
        Ok(json) => {
            if json == "undefined" {
                value::encode_undefined()
            } else {
                ctx.store_string(&json)
            }
        }
        Err(exc) => exc,
    }
}

/// 序列化 JSON 属性（核心递归 impl，含 cycle、toJSON、replacer、pretty-print）。
#[allow(clippy::too_many_arguments)]
async fn serialize_json_property<E: ExecContext>(
    ctx: &mut E,
    key: &str,
    val: Value,
    holder: Value,
    replacer_is_fn: bool,
    replacer_fn: Option<Value>,
    property_list: Option<&[String]>,
    stack: &mut Vec<Value>,
    gap: &str,
    current_indent: &str,
) -> Result<String, Value> {
    let mut value = get_to_json(ctx, key, val).await;
    if value::is_exception(value) {
        return Err(value);
    }
    let mut replacer_returned_undefined = false;
    if let Some(rf) = replacer_fn.filter(|_| replacer_is_fn) {
        let key_str = ctx.store_string(key);
        let args = [key_str, value];
        match ctx.call_js_async(rf, holder, &args).await {
            Ok(new_val) => {
                if value::is_exception(new_val) {
                    return Err(new_val);
                }
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
        let f = normalize_negative_zero(value::decode_f64(value));
        return Ok(if !f.is_finite() {
            "null".to_string()
        } else {
            format_number_js(f)
        });
    }
    if value::is_undefined(value) {
        if replacer_returned_undefined || key.is_empty() {
            return Ok("undefined".to_string());
        }
        return Ok("null".to_string());
    }
    if value::is_callable(value) || value::is_symbol(value) {
        return Ok("undefined".to_string());
    }
    if value::is_bigint(value) {
        return Err(ctx.make_type_error("Do not know how to serialize a BigInt"));
    }
    if value::is_string(value) {
        return Ok(ctx.get_runtime_string(value).to_json_quoted());
    }
    if value::is_bool(value) {
        return Ok(value::decode_bool(value).to_string());
    }
    if value::is_null(value) {
        return Ok("null".to_string());
    }

    let next_indent = if gap.is_empty() {
        String::new()
    } else {
        format!("{current_indent}{gap}")
    };

    if value::is_array(value) {
        if stack.contains(&value) {
            return Err(ctx.make_type_error("Converting circular structure to JSON"));
        }
        stack.push(value);
        let Some(len) = ctx.array_read_length(value) else {
            stack.pop();
            return Ok("null".to_string());
        };
        let mut parts = Vec::with_capacity(len as usize);
        for i in 0..len {
            let elem = ctx
                .array_elem_at(value, i)
                .unwrap_or_else(value::encode_undefined);
            let s = match Box::pin(serialize_json_property(
                ctx,
                &i.to_string(),
                elem,
                value,
                replacer_is_fn,
                replacer_fn,
                property_list,
                stack,
                gap,
                &next_indent,
            ))
            .await
            {
                Ok(s) => s,
                Err(exc) => return Err(exc),
            };
            parts.push(if s == "undefined" {
                "null".to_string()
            } else {
                s
            });
        }
        stack.pop();
        return Ok(if parts.is_empty() {
            "[]".to_string()
        } else if gap.is_empty() {
            format!("[{}]", parts.join(","))
        } else {
            let inner = parts.join(&format!(",\n{next_indent}"));
            format!("[\n{next_indent}{inner}\n{current_indent}]")
        });
    }

    if value::is_object(value) {
        if stack.contains(&value) {
            return Err(ctx.make_type_error("Converting circular structure to JSON"));
        }
        stack.push(value);
        let Some(slots) = ctx.own_enumerable_data_slots(value) else {
            stack.pop();
            return Ok("null".to_string());
        };

        let mut pairs = Vec::new();
        if let Some(property_list) = property_list {
            for name in property_list {
                if let Some(prop_val) = ctx.read_property_for_render(value, name) {
                    if value::is_undefined(prop_val) {
                        continue;
                    }
                    let s = match Box::pin(serialize_json_property(
                        ctx,
                        name,
                        prop_val,
                        value,
                        replacer_is_fn,
                        replacer_fn,
                        Some(property_list),
                        stack,
                        gap,
                        &next_indent,
                    ))
                    .await
                    {
                        Ok(s) => s,
                        Err(exc) => return Err(exc),
                    };
                    if s != "undefined" {
                        let colon = if gap.is_empty() { ":" } else { ": " };
                        pairs.push(format!("{}{}{}", json_escape_string(name), colon, s));
                    }
                }
            }
        } else {
            for (name_id, prop_val) in slots {
                let Some(name) = ctx.property_key_string(name_id) else {
                    // Symbol 键或不可反查的 name_id：跳过
                    continue;
                };
                let s = match Box::pin(serialize_json_property(
                    ctx,
                    &name,
                    prop_val,
                    value,
                    replacer_is_fn,
                    replacer_fn,
                    None,
                    stack,
                    gap,
                    &next_indent,
                ))
                .await
                {
                    Ok(s) => s,
                    Err(exc) => return Err(exc),
                };
                if s != "undefined" {
                    let colon = if gap.is_empty() { ":" } else { ": " };
                    pairs.push(format!("{}{}{}", json_escape_string(&name), colon, s));
                }
            }
        }

        stack.pop();
        return Ok(if pairs.is_empty() {
            "{}".to_string()
        } else if gap.is_empty() {
            format!("{{{}}}", pairs.join(","))
        } else {
            let inner = pairs.join(&format!(",\n{next_indent}"));
            format!("{{\n{next_indent}{inner}\n{current_indent}}}")
        });
    }

    Ok("null".to_string())
}
