use anyhow::Result;
use icu_normalizer::{ComposingNormalizerBorrowed, DecomposingNormalizerBorrowed};
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::runtime_host_helpers::make_range_error_exception;
use crate::*;

/// String value consisting of a single UTF-16 code unit (ECMAScript §22.1.3.1).
pub(crate) fn string_from_utf16_code_unit(unit: u16) -> String {
    if !(0xD800..=0xDFFF).contains(&unit) {
        if let Some(ch) = char::from_u32(unit as u32) {
            return ch.to_string();
        }
    }
    let bytes = if unit < 0x800 {
        vec![0xC0 | ((unit >> 6) as u8), 0x80 | ((unit & 0x3F) as u8)]
    } else {
        vec![
            0xE0 | ((unit >> 12) as u8),
            0x80 | (((unit >> 6) & 0x3F) as u8),
            0x80 | ((unit & 0x3F) as u8),
        ]
    };
    unsafe { String::from_utf8_unchecked(bytes) }
}

/// ECMAScript §22.1.3.15：按 form 对字符串做 Unicode 规范化。
fn normalize_string_by_form(s: &str, form: &str) -> Result<String, &'static str> {
    match form {
        "NFC" => Ok(ComposingNormalizerBorrowed::new_nfc()
            .normalize(s)
            .into_owned()),
        "NFD" => Ok(DecomposingNormalizerBorrowed::new_nfd()
            .normalize(s)
            .into_owned()),
        "NFKC" => Ok(ComposingNormalizerBorrowed::new_nfkc()
            .normalize(s)
            .into_owned()),
        "NFKD" => Ok(DecomposingNormalizerBorrowed::new_nfkd()
            .normalize(s)
            .into_owned()),
        _ => Err("The normalization form should be one of NFC, NFD, NFKC, NFKD"),
    }
}

pub(crate) fn define_string_methods(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    fn to_f64_or(val: i64, default: f64) -> f64 {
        if value::is_f64(val) {
            value::decode_f64(val)
        } else {
            default
        }
    }

    fn to_uint16_number(n: f64) -> u16 {
        if !n.is_finite() || n == 0.0 {
            return 0;
        }
        let v = n.trunc().rem_euclid(65536.0);
        v as u16
    }

    fn to_uint32_number(n: f64) -> u32 {
        if !n.is_finite() || n == 0.0 {
            return 0;
        }
        let v = n.trunc().rem_euclid(4294967296.0);
        v as u32
    }

    fn to_uint16_caller(caller: &mut Caller<'_, RuntimeState>, val: i64) -> u16 {
        to_uint16_number(value::decode_f64(to_number(caller, val)))
    }

    fn to_uint32_caller(caller: &mut Caller<'_, RuntimeState>, val: i64) -> u32 {
        to_uint32_number(value::decode_f64(to_number(caller, val)))
    }

    fn js_utf16_units(s: &str) -> Vec<u16> {
        let mut units = Vec::new();
        let b = s.as_bytes();
        let mut i = 0usize;
        while i < b.len() {
            if b[i] < 0x80 {
                units.push(b[i] as u16);
                i += 1;
                continue;
            }
            if i + 2 < b.len() && (b[i] & 0xF0) == 0xE0 {
                let unit = (((b[i] as u32 & 0x0F) << 12)
                    | ((b[i + 1] as u32 & 0x3F) << 6)
                    | (b[i + 2] as u32 & 0x3F)) as u16;
                if (0xD800..=0xDFFF).contains(&unit) {
                    units.push(unit);
                    i += 3;
                    continue;
                }
            }
            if i + 1 < b.len() && (b[i] & 0xE0) == 0xC0 {
                let unit = (((b[i] as u32 & 0x1F) << 6) | (b[i + 1] as u32 & 0x3F)) as u16;
                if (0xD800..=0xDFFF).contains(&unit) {
                    units.push(unit);
                    i += 2;
                    continue;
                }
            }
            if let Some(ch) = std::str::from_utf8(&b[i..])
                .ok()
                .and_then(|tail| tail.chars().next())
            {
                let cp = ch as u32;
                if cp > 0xFFFF {
                    let base = cp - 0x10000;
                    units.push((0xD800 + (base >> 10)) as u16);
                    units.push((0xDC00 + (base & 0x3FF)) as u16);
                } else {
                    units.push(cp as u16);
                }
                i += ch.len_utf8();
                continue;
            }
            i += 1;
        }
        units
    }

    fn utf16_code_unit_at_stored(s: &str, utf16_idx: usize) -> Option<u16> {
        js_utf16_units(s).get(utf16_idx).copied()
    }

    fn js_utf16_len(s: &str) -> usize {
        js_utf16_units(s).len()
    }

    fn concat_arg_to_string(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
        if value::is_string(val) {
            get_string_value(caller, val)
        } else {
            render_value(caller, val).unwrap_or_default()
        }
    }

    fn is_valid_code_point(cp: u32) -> bool {
        cp <= 0x10FFFF && !(0xD800..=0xDFFF).contains(&cp)
    }

    fn push_utf16_code_unit(result: &mut String, unit: u16) {
        result.push_str(&string_from_utf16_code_unit(unit));
    }

    // ── string_at ──
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, index: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = js_utf16_len(&s) as i64;
            let idx = to_f64_or(index, 0.0);
            let mut i = idx as i64;
            if idx < 0.0 {
                i += len;
            }
            if i < 0 || i >= len {
                return value::encode_undefined();
            }
            let unit = utf16_code_unit_at_stored(&s, i as usize).unwrap_or(0);
            store_runtime_string(&caller, string_from_utf16_code_unit(unit))
        },
    );
    linker.define(&mut store, "env", "string_at", f)?;
    // string_char_at
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = js_utf16_len(&s);
            let p = to_uint32_caller(&mut caller, pos) as usize;
            if p >= len {
                return store_runtime_string(&caller, String::new());
            }
            let unit = utf16_code_unit_at_stored(&s, p).unwrap_or(0);
            store_runtime_string(&caller, string_from_utf16_code_unit(unit))
        },
    );
    linker.define(&mut store, "env", "string_char_at", f)?;
    // string_char_code_at
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = js_utf16_len(&s);
            let p = to_uint32_caller(&mut caller, pos) as usize;
            if p >= len {
                return value::encode_f64(f64::NAN);
            }
            let unit = utf16_code_unit_at_stored(&s, p).unwrap_or(0);
            value::encode_f64(unit as f64)
        },
    );
    linker.define(&mut store, "env", "string_char_code_at", f)?;
    // string_code_point_at
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = js_utf16_len(&s);
            let p = to_uint32_caller(&mut caller, pos) as usize;
            if p >= len {
                return value::encode_undefined();
            }
            let units = js_utf16_units(&s);
            let Some(&unit) = units.get(p) else {
                return value::encode_undefined();
            };
            if (0xD800..=0xDBFF).contains(&unit) && p + 1 < units.len() {
                let lo = units[p + 1];
                if (0xDC00..=0xDFFF).contains(&lo) {
                    let cp = 0x10000 + (((unit as u32 - 0xD800) << 10) | (lo as u32 - 0xDC00));
                    return value::encode_f64(cp as f64);
                }
            }
            value::encode_f64(unit as f64)
        },
    );
    linker.define(&mut store, "env", "string_code_point_at", f)?;
    // string_proto_concat
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            if value::is_array(this_val) {
                return super::array_object::array_concat_args(
                    &mut caller,
                    this_val,
                    args_base,
                    args_count,
                );
            }
            let mut result = get_string_value(&mut caller, this_val);
            let parts: Vec<String> = (0..args_count as u32)
                .map(|i| {
                    let arg = read_shadow_arg(&mut caller, args_base, i);
                    concat_arg_to_string(&mut caller, arg)
                })
                .collect();
            for p in parts {
                result.push_str(&p);
            }
            store_runtime_string(&caller, result)
        },
    );
    linker.define(&mut store, "env", "string_proto_concat", f)?;
    // string_ends_with
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, end_pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let search_str = get_string_value(&mut caller, search);
            let len = utf16_len(&s);
            let end_utf16 = if end_pos == value::encode_undefined() {
                len
            } else {
                (to_f64_or(end_pos, 0.0) as usize).min(len)
            };
            let end_byte = utf16_index_to_byte_offset(&s, end_utf16);
            value::encode_bool(if search_str.is_empty() {
                true
            } else {
                s[..end_byte].ends_with(&search_str)
            })
        },
    );
    linker.define(&mut store, "env", "string_ends_with", f)?;
    // string_includes
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, pos: i64| -> i64 {
            if value::is_array(receiver)
                && let Some(ptr) = resolve_array_ptr(&mut caller, receiver)
            {
                let len = read_array_length(&mut caller, ptr).unwrap_or(0);
                return super::array_object::array_includes_from(
                    &mut caller,
                    ptr,
                    len,
                    search,
                    pos,
                );
            }
            let s = get_string_value(&mut caller, receiver);
            let start_utf16 = if pos == value::encode_undefined() {
                0
            } else {
                (to_f64_or(pos, 0.0) as usize).min(utf16_len(&s))
            };
            let start_byte = utf16_index_to_byte_offset(&s, start_utf16);
            value::encode_bool(s[start_byte..].contains(&get_string_value(&mut caller, search)))
        },
    );
    linker.define(&mut store, "env", "string_includes", f)?;
    // string_index_of
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, pos: i64| -> i64 {
            if value::is_array(receiver)
                && let Some(ptr) = resolve_array_ptr(&mut caller, receiver)
            {
                let len = read_array_length(&mut caller, ptr).unwrap_or(0);
                return super::array_object::array_index_of_from(
                    &mut caller,
                    ptr,
                    len,
                    search,
                    pos,
                );
            }
            let s = get_string_value(&mut caller, receiver);
            let search_str = get_string_value(&mut caller, search);
            let start_utf16 = if pos == value::encode_undefined() {
                0
            } else {
                (to_f64_or(pos, 0.0) as usize).min(utf16_len(&s))
            };
            let start_byte = utf16_index_to_byte_offset(&s, start_utf16);
            match if search_str.is_empty() {
                Some(start_byte)
            } else {
                s[start_byte..].find(&search_str).map(|i| start_byte + i)
            } {
                Some(byte_idx) => {
                    value::encode_f64(byte_offset_to_utf16_index(&s, byte_idx) as f64)
                }
                None => value::encode_f64(-1.0),
            }
        },
    );
    linker.define(&mut store, "env", "string_index_of", f)?;
    // string_last_index_of
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, pos: i64| -> i64 {
            // 数组接收者回退到 array_last_index_of_from（同 string_index_of）。
            if value::is_array(receiver)
                && let Some(ptr) = resolve_array_ptr(&mut caller, receiver)
            {
                let len = read_array_length(&mut caller, ptr).unwrap_or(0);
                let from_index = if pos == value::encode_undefined() {
                    value::encode_f64((len as i64 - 1) as f64)
                } else {
                    pos
                };
                return super::array_object::array_last_index_of_from(
                    &mut caller,
                    ptr,
                    len,
                    search,
                    from_index,
                );
            }
            let s = get_string_value(&mut caller, receiver);
            let search_str = get_string_value(&mut caller, search);
            let len_utf16 = utf16_len(&s);
            if search_str.is_empty() {
                let end_utf16 = if pos == value::encode_undefined() {
                    len_utf16
                } else {
                    (to_f64_or(pos, 0.0) as usize).min(len_utf16)
                };
                return value::encode_f64(end_utf16 as f64);
            }
            let end_utf16 = if pos == value::encode_undefined() {
                len_utf16
            } else {
                (to_f64_or(pos, 0.0) as usize).min(len_utf16)
            };
            let end_byte = utf16_index_to_byte_offset(&s, end_utf16);
            match s[..end_byte].rfind(&search_str) {
                Some(byte_idx) => {
                    value::encode_f64(byte_offset_to_utf16_index(&s, byte_idx) as f64)
                }
                None => value::encode_f64(-1.0),
            }
        },
    );
    linker.define(&mut store, "env", "string_last_index_of", f)?;
    // string_match_all 已移至 reentrant_string_async（需异步 reentry 以支持 @@matchAll 自定义匹配器）
    // string_pad_end
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         receiver: i64,
         target_len: i64,
         pad_str_val: i64|
         -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = utf16_len(&s);
            let target = if value::is_f64(target_len) {
                to_f64_or(target_len, 0.0) as usize
            } else {
                0
            };
            if target <= len {
                return store_runtime_string(&caller, s);
            }
            let pad_str = if pad_str_val == value::encode_undefined() {
                " ".to_string()
            } else {
                let p = get_string_value(&mut caller, pad_str_val);
                if p.is_empty() { " ".to_string() } else { p }
            };
            let pad_chars: Vec<char> = pad_str.chars().collect();
            let mut result = s.clone();
            for i in len..target {
                result.push(pad_chars[(i - len) % pad_chars.len()]);
            }
            store_runtime_string(&caller, result)
        },
    );
    linker.define(&mut store, "env", "string_pad_end", f)?;
    // string_pad_start
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         receiver: i64,
         target_len: i64,
         pad_str_val: i64|
         -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = utf16_len(&s);
            let target = if value::is_f64(target_len) {
                to_f64_or(target_len, 0.0) as usize
            } else {
                0
            };
            if target <= len {
                return store_runtime_string(&caller, s);
            }
            let pad_str = if pad_str_val == value::encode_undefined() {
                " ".to_string()
            } else {
                let p = get_string_value(&mut caller, pad_str_val);
                if p.is_empty() { " ".to_string() } else { p }
            };
            let pad_chars: Vec<char> = pad_str.chars().collect();
            let mut result = String::new();
            for i in 0..(target - len) {
                result.push(pad_chars[i % pad_chars.len()]);
            }
            result.push_str(&s);
            store_runtime_string(&caller, result)
        },
    );
    linker.define(&mut store, "env", "string_pad_start", f)?;
    // string_repeat
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, count: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let c = to_f64_or(count, 0.0);
            if c < 0.0 || c.is_infinite() {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) =
                    Some("RangeError: Invalid count value".to_string());
                return value::encode_undefined();
            }
            store_runtime_string(&caller, s.repeat(c as usize))
        },
    );
    linker.define(&mut store, "env", "string_repeat", f)?;
    // string_replace_all
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, replace: i64| -> i64 {
            if value::is_regexp(search) {
                let handle = value::decode_regexp_handle(search);
                let is_global = {
                    let table = caller
                        .data()
                        .regex_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    match table.get(handle as usize) {
                        Some(e) => e.flags.contains('g'),
                        None => false,
                    }
                };
                if !is_global {
                    *caller
                        .data()
                        .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                        "TypeError: String.prototype.replaceAll called with a non-global RegExp argument"
                            .to_string(),
                    );
                    return value::encode_undefined();
                }
                let rt = tokio::runtime::Handle::current();
                return rt.block_on(super::reentrant_async::string_replace_async_body(
                    caller, receiver, search, replace,
                ));
            }
            let s = get_string_value(&mut caller, receiver);
            let search_str = get_string_value(&mut caller, search);
            let replace_str = get_string_value(&mut caller, replace);
            store_runtime_string(&caller, s.replace(&search_str, &replace_str))
        },
    );
    linker.define(&mut store, "env", "string_replace_all", f)?;
    // string_slice
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, start: i64, end: i64| -> i64 {
            if value::is_array(receiver) {
                return super::array_object::array_slice_range(&mut caller, receiver, start, end);
            }
            let s = get_string_value(&mut caller, receiver);
            let len = utf16_len(&s) as i64;
            let si = if value::is_f64(start) {
                let v = to_f64_or(start, 0.0) as i64;
                if v < 0 { (v + len).max(0) } else { v.min(len) }
            } else {
                0
            };
            let ei = if end == value::encode_undefined() {
                len
            } else if value::is_f64(end) {
                let v = to_f64_or(end, 0.0) as i64;
                if v < 0 { (v + len).max(0) } else { v.min(len) }
            } else {
                0
            };
            if si >= ei {
                return store_runtime_string(&caller, String::new());
            }
            let start_byte = utf16_index_to_byte_offset(&s, si as usize);
            let end_byte = utf16_index_to_byte_offset(&s, ei as usize);
            store_runtime_string(&caller, s[start_byte..end_byte].to_string())
        },
    );
    linker.define(&mut store, "env", "string_slice", f)?;
    // string_starts_with
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let start_utf16 = if pos == value::encode_undefined() {
                0
            } else {
                (to_f64_or(pos, 0.0) as usize).min(utf16_len(&s))
            };
            let start_byte = utf16_index_to_byte_offset(&s, start_utf16);
            value::encode_bool(s[start_byte..].starts_with(&get_string_value(&mut caller, search)))
        },
    );
    linker.define(&mut store, "env", "string_starts_with", f)?;
    // string_substring
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, start: i64, end: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = utf16_len(&s) as i64;
            let s1 = if value::is_f64(start) {
                (to_f64_or(start, 0.0) as i64).max(0).min(len)
            } else {
                0
            };
            let e1 = if end == value::encode_undefined() {
                len
            } else {
                (to_f64_or(end, 0.0) as i64).max(0).min(len)
            };
            let (lo, hi) = if s1 < e1 { (s1, e1) } else { (e1, s1) };
            if lo >= hi {
                return store_runtime_string(&caller, String::new());
            }
            let lo_byte = utf16_index_to_byte_offset(&s, lo as usize);
            let hi_byte = utf16_index_to_byte_offset(&s, hi as usize);
            store_runtime_string(&caller, s[lo_byte..hi_byte].to_string())
        },
    );
    linker.define(&mut store, "env", "string_substring", f)?;
    // string_normalize
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, form_val: i64, _unused: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let form = if form_val == value::encode_undefined() {
                "NFC".to_string()
            } else {
                get_string_value(&mut caller, form_val)
            };
            match normalize_string_by_form(&s, &form) {
                Ok(out) => store_runtime_string(&caller, out),
                Err(msg) => make_range_error_exception(&mut caller, msg),
            }
        },
    );
    linker.define(&mut store, "env", "string_normalize", f)?;
    // string_to_lower_case
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver).to_lowercase();
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_to_lower_case", f)?;
    // string_to_upper_case
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver).to_uppercase();
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_to_upper_case", f)?;
    // string_trim
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver).trim().to_string();
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_trim", f)?;
    // string_trim_end
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver)
                .trim_end()
                .to_string();
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_trim_end", f)?;
    // string_trim_start
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver)
                .trim_start()
                .to_string();
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_trim_start", f)?;
    // string_to_string
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            if value::is_string(receiver) {
                let s = get_string_value(&mut caller, receiver);
                store_runtime_string(&caller, s)
            } else {
                obj_proto_to_string_impl(&mut caller, receiver)
            }
        },
    );
    linker.define(&mut store, "env", "string_to_string", f)?;
    // string_value_of
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_value_of", f)?;
    // string_iterator
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let mut iters = caller
                .data()
                .iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let handle = iters.len() as u32;
            iters.push(IteratorState::StringIter {
                data: s.into_bytes(),
                byte_pos: 0,
            });
            value::encode_handle(value::TAG_ITERATOR, handle)
        },
    );
    linker.define(&mut store, "env", "string_iterator", f)?;
    // string_from_char_code
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         _this: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let mut result = String::new();
            for i in 0..args_count as u32 {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                let code = to_uint16_caller(&mut caller, arg);
                push_utf16_code_unit(&mut result, code);
            }
            store_runtime_string(&caller, result)
        },
    );
    linker.define(&mut store, "env", "string_from_char_code", f)?;
    // string_from_code_point
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         _this: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let mut result = String::new();
            for i in 0..args_count as u32 {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                let code = to_uint32_caller(&mut caller, arg);
                if !is_valid_code_point(code) {
                    return make_range_error_exception(&mut caller, "Invalid code point");
                }
                if let Some(c) = char::from_u32(code) {
                    result.push(c);
                }
            }
            store_runtime_string(&caller, result)
        },
    );
    linker.define(&mut store, "env", "string_from_code_point", f)?;

    // ── promise_create ──
    Ok(())
}
