use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::*;

pub(crate) fn define_string_methods(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    fn utf16_len(s: &str) -> usize {
        s.chars()
            .map(|c| if c as u32 > 0xFFFF { 2 } else { 1 })
            .sum()
    }

    fn utf16_index_to_byte_offset(s: &str, utf16_idx: usize) -> usize {
        let mut utf16_count = 0usize;
        for (byte_off, ch) in s.char_indices() {
            if utf16_count >= utf16_idx {
                return byte_off;
            }
            utf16_count += if ch as u32 > 0xFFFF { 2 } else { 1 };
        }
        s.len()
    }

    fn byte_offset_to_utf16_index(s: &str, byte_off: usize) -> usize {
        let mut utf16_count = 0usize;
        for (off, ch) in s.char_indices() {
            if off >= byte_off {
                break;
            }
            utf16_count += if ch as u32 > 0xFFFF { 2 } else { 1 };
        }
        utf16_count
    }

    fn to_f64_or(val: i64, default: f64) -> f64 {
        if value::is_f64(val) {
            value::decode_f64(val)
        } else {
            default
        }
    }

    fn to_uint16(val: i64) -> u16 {
        if value::is_f64(val) {
            value::decode_f64(val) as u16
        } else {
            0
        }
    }

    fn to_uint32(val: i64) -> u32 {
        if value::is_f64(val) {
            value::decode_f64(val) as u32
        } else {
            0
        }
    }
    /// 从 regress::Match 构建 RegExp 执行结果数组。
    // 返回的数组包含 .index, .input, .groups 属性；
    // 若 flags 包含 'd' 则额外设置 .indices 及 indices.groups。
    // named_groups() 只 collect 一次，供 .groups 和 .indices.groups 复用。
    fn build_match_result(
        caller: &mut Caller<'_, RuntimeState>,
        m: &regress::Match,
        s: &str,
        group_count: u32,
        flags: &str,
    ) -> i64 {
        let arr = alloc_array(caller, group_count);
        let Some(arr_ptr) = resolve_array_ptr(caller, arr) else {
            return value::encode_null();
        };
        for i in 0..group_count {
            let elem = if let Some(range) = m.group(i as usize) {
                let group_str = &s[range];
                store_runtime_string(caller, group_str.to_string())
            } else {
                value::encode_undefined()
            };
            write_array_elem(caller, arr_ptr, i as u32, elem);
        }
        write_array_length(caller, arr_ptr, group_count);
        // .index — 使用 m.start() 保持一致
        let index_val = value::encode_f64(m.start() as f64);
        let _ = define_host_data_property_from_caller(caller, arr_ptr as i64, "index", index_val);
        // .input
        let input_val = store_runtime_string(caller, s.to_string());
        let _ = define_host_data_property_from_caller(caller, arr_ptr as i64, "input", input_val);
        // .groups
        let named: Vec<(&str, Option<std::ops::Range<usize>>)> = m.named_groups().collect();
        if !named.is_empty() {
            let groups_obj = {
                let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
                alloc_host_null_proto_object(caller, &_wjsm_env, named.len() as u32)
            };
            for (name, range) in &named {
                let val = match range {
                    Some(r) => store_runtime_string(caller, s[r.clone()].to_string()),
                    None => value::encode_undefined(),
                };
                let _ = define_host_data_property_from_caller(caller, groups_obj, name, val);
            }
            let _ =
                define_host_data_property_from_caller(caller, arr_ptr as i64, "groups", groups_obj);
        } else {
            let _ = define_host_data_property_from_caller(
                caller,
                arr_ptr as i64,
                "groups",
                value::encode_undefined(),
            );
        }
        // .indices（仅 d 标志）
        if flags.contains('d') {
            let indices_arr = alloc_array(caller, group_count);
            let Some(indices_ptr) = resolve_array_ptr(caller, indices_arr) else {
                return value::encode_null();
            };
            for i in 0..group_count {
                let elem = match m.group(i as usize) {
                    Some(range) => {
                        let pair = alloc_array(caller, 2);
                        let pair_ptr = resolve_array_ptr(caller, pair).unwrap_or(0);
                        write_array_elem(
                            caller,
                            pair_ptr,
                            0,
                            value::encode_f64(range.start as f64),
                        );
                        write_array_elem(caller, pair_ptr, 1, value::encode_f64(range.end as f64));
                        write_array_length(caller, pair_ptr, 2);
                        pair
                    }
                    None => value::encode_undefined(),
                };
                write_array_elem(caller, indices_ptr, i as u32, elem);
            }
            write_array_length(caller, indices_ptr, group_count);
            if !named.is_empty() {
                let ig = {
                    let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
                    alloc_host_null_proto_object(caller, &_wjsm_env, named.len() as u32)
                };
                for (name, range) in &named {
                    let val = match range {
                        Some(r) => {
                            let pair = alloc_array(caller, 2);
                            let pair_ptr = resolve_array_ptr(caller, pair).unwrap_or(0);
                            write_array_elem(
                                caller,
                                pair_ptr,
                                0,
                                value::encode_f64(r.start as f64),
                            );
                            write_array_elem(caller, pair_ptr, 1, value::encode_f64(r.end as f64));
                            write_array_length(caller, pair_ptr, 2);
                            pair
                        }
                        None => value::encode_undefined(),
                    };
                    let _ = define_host_data_property_from_caller(caller, ig, name, val);
                }
                let _ =
                    define_host_data_property_from_caller(caller, indices_ptr as i64, "groups", ig);
            } else {
                let _ = define_host_data_property_from_caller(
                    caller,
                    indices_ptr as i64,
                    "groups",
                    value::encode_undefined(),
                );
            }
            let _ = define_host_data_property_from_caller(
                caller,
                arr_ptr as i64,
                "indices",
                indices_arr,
            );
        }
        arr
    }

    // ── string_at ──
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, index: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = utf16_len(&s) as i64;
            let idx = to_f64_or(index, 0.0);
            let mut i = idx as i64;
            if idx < 0.0 {
                i += len;
            }
            if i < 0 || i >= len {
                return value::encode_undefined();
            }
            let byte_off = utf16_index_to_byte_offset(&s, i as usize);
            let ch = s[byte_off..]
                .chars()
                .next()
                .map(|c| c.to_string())
                .unwrap_or_default();
            store_runtime_string(&caller, ch)
        },
    );
    linker.define(&mut store, "env", "string_at", f)?;
    // string_char_at
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = utf16_len(&s);
            let p = to_uint32(pos) as usize;
            if p >= len {
                return value::encode_f64(f64::NAN);
            }
            let byte_off = utf16_index_to_byte_offset(&s, p);
            let ch = s[byte_off..]
                .chars()
                .next()
                .map(|c| c.to_string())
                .unwrap_or_default();
            store_runtime_string(&caller, ch)
        },
    );
    linker.define(&mut store, "env", "string_char_at", f)?;
    // string_char_code_at
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = utf16_len(&s);
            let p = to_uint32(pos) as usize;
            if p >= len {
                return value::encode_f64(f64::NAN);
            }
            let byte_off = utf16_index_to_byte_offset(&s, p);
            let ch = s[byte_off..].chars().next();
            match ch {
                Some(c) if (c as u32) <= 0xFFFF => value::encode_f64(c as u32 as f64),
                Some(c) => {
                    let code = c as u32;
                    let hi = (((code - 0x10000) >> 10) + 0xD800) as u16;
                    value::encode_f64(hi as f64)
                }
                None => value::encode_f64(f64::NAN),
            }
        },
    );
    linker.define(&mut store, "env", "string_char_code_at", f)?;
    // string_code_point_at
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = utf16_len(&s);
            let p = to_uint32(pos) as usize;
            if p >= len {
                return value::encode_undefined();
            }
            let byte_off = utf16_index_to_byte_offset(&s, p);
            let ch = s[byte_off..].chars().next();
            match ch {
                Some(c) => value::encode_f64(c as u32 as f64),
                None => value::encode_undefined(),
            }
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
            let mut result = get_string_value(&mut caller, this_val);
            let parts: Vec<String> = (0..args_count as u32)
                .map(|i| {
                    let arg = read_shadow_arg(&mut caller, args_base, i);
                    get_string_value(&mut caller, arg)
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
    // string_match_all
    let string_match_all_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            if args_count < 1 {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") = Some(
                    "TypeError: String.prototype.matchAll requires a regexp argument".to_string(),
                );
                return value::encode_undefined();
            }
            let regexp = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_regexp(regexp) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: String.prototype.matchAll called with non-RegExp".to_string());
                return value::encode_undefined();
            }
            let handle = value::decode_regexp_handle(regexp);
            let is_global = {
                let table = caller.data().regex_table.lock().unwrap();
                match table.get(handle as usize) {
                    Some(e) => e.flags.contains('g'),
                    None => {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .expect("runtime error mutex") = Some(
                            "TypeError: String.prototype.matchAll called with a non-global RegExp"
                                .to_string(),
                        );
                        return value::encode_undefined();
                    }
                }
            };
            if !is_global {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") = Some(
                    "TypeError: String.prototype.matchAll called with a non-global RegExp"
                        .to_string(),
                );
                return value::encode_undefined();
            }
            {
                let mut table = caller.data().regex_table.lock().unwrap();
                if let Some(e) = table.get_mut(handle as usize) {
                    e.last_index = 0;
                }
            }
            let s = get_string_value(&mut caller, this_val);
            let entry = {
                let table = caller.data().regex_table.lock().unwrap();
                match table.get(handle as usize) {
                    Some(e) => e.clone(),
                    None => return value::encode_undefined(),
                }
            };
            let mut results = Vec::new();
            for m in entry.compiled.find_iter(&s) {
                let arr = build_match_result(
                    &mut caller,
                    &m,
                    &s,
                    (m.captures.len() + 1) as u32,
                    &entry.flags,
                );
                results.push(arr);
                if results.len() > 10000 {
                    break;
                }
            }
            {
                let mut table = caller.data().regex_table.lock().unwrap();
                if let Some(e) = table.get_mut(handle as usize) {
                    e.last_index = 0;
                }
            }
            let result_arr = alloc_array(&mut caller, results.len() as u32);
            if let Some(result_ptr) = resolve_array_ptr(&mut caller, result_arr) {
                for (i, &val) in results.iter().enumerate() {
                    write_array_elem(&mut caller, result_ptr, i as u32, val);
                }
                write_array_length(&mut caller, result_ptr, results.len() as u32);
            }
            result_arr
        },
    );
    linker.define(&mut store, "env", "string_match_all", string_match_all_fn)?;
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
                    .expect("runtime error mutex") =
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
                *caller.data().runtime_error.lock().expect("runtime error mutex") = Some("TypeError: String.prototype.replaceAll called with a non-global RegExp argument".to_string());
                return value::encode_undefined();
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
            let mut iters = caller.data().iterators.lock().expect("iterators mutex");
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
                let code = to_uint16(arg) as u32;
                if let Some(c) = char::from_u32(code) {
                    result.push(c);
                }
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
                let code = to_uint32(arg);
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
