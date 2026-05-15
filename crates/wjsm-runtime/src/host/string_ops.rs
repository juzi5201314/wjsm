use wasmtime::*;
use wjsm_ir::value;

use crate::types::*;
use crate::runtime::*;

fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| if c as u32 > 0xFFFF { 2 } else { 1 }).sum()
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
    if value::is_f64(val) { value::decode_f64(val) } else { default }
}

fn to_uint16(val: i64) -> u16 {
    if value::is_f64(val) { value::decode_f64(val) as u16 } else { 0 }
}

fn to_uint32(val: i64) -> u32 {
    if value::is_f64(val) { value::decode_f64(val) as u32 } else { 0 }
}

// ── Import 166: string_at(i64, i64) -> i64 ──────────────────────────────────

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let string_concat = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            if value::is_string(a) || value::is_string(b) {
                // 至少一个操作数是字符串 → 执行字符串连接
                let a_s = if value::is_string(a) {
                    read_value_string_bytes(&mut caller, a).unwrap_or_default()
                } else {
                    render_value(&mut caller, a)
                        .unwrap_or_default()
                        .into_bytes()
                };
                let b_s = if value::is_string(b) {
                    read_value_string_bytes(&mut caller, b).unwrap_or_default()
                } else {
                    render_value(&mut caller, b)
                        .unwrap_or_default()
                        .into_bytes()
                };
                let mut result = a_s;
                result.extend(b_s);
                let s = String::from_utf8(result).unwrap_or_default();
                store_runtime_string(&caller, s)
            } else {
                // 都不是 string → 返回 undefined 作为哨兵值，由 WASM 后端走数值加法
                value::encode_undefined()
            }
        },
    );

    // ── Import 17: string_concat_va(i32, i32) → i64 ────────────────────────

    let string_concat_va = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
            let mut result = Vec::new();
            for i in 0..args_count as u32 {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                let s = render_value(&mut caller, arg).unwrap_or_default();
                result.extend(s.into_bytes());
            }
            let s = String::from_utf8(result).unwrap_or_default();
            store_runtime_string(&caller, s)
        },
    );

    // ── Import 18: define_property(i64, i32, i64) → () ────────────────────

    let string_at_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, index: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let len = utf16_len(&s) as i64;
        let idx = to_f64_or(index, 0.0);
        let mut i = idx as i64;
        if idx < 0.0 { i += len; }
        if i < 0 || i >= len { return value::encode_undefined(); }
        let byte_off = utf16_index_to_byte_offset(&s, i as usize);
        let ch = s[byte_off..].chars().next().map(|c| c.to_string()).unwrap_or_default();
        store_runtime_string(&caller, ch)
    });
    // Import 167: string_char_at(i64, i64) -> i64

    let string_char_at_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, pos: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let len = utf16_len(&s);
        let p = to_uint32(pos) as usize;
        if p >= len { return value::encode_f64(f64::NAN); }
        let byte_off = utf16_index_to_byte_offset(&s, p);
        let ch = s[byte_off..].chars().next().map(|c| c.to_string()).unwrap_or_default();
        store_runtime_string(&caller, ch)
    });
    // Import 168: string_char_code_at(i64, i64) -> i64

    let string_char_code_at_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, pos: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let len = utf16_len(&s);
        let p = to_uint32(pos) as usize;
        if p >= len { return value::encode_f64(f64::NAN); }
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
    });
    // Import 169: string_code_point_at(i64, i64) -> i64

    let string_code_point_at_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, pos: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let len = utf16_len(&s);
        let p = to_uint32(pos) as usize;
        if p >= len { return value::encode_undefined(); }
        let byte_off = utf16_index_to_byte_offset(&s, p);
        let ch = s[byte_off..].chars().next();
        match ch {
            Some(c) => value::encode_f64(c as u32 as f64),
            None => value::encode_undefined(),
        }
    });
    // Import 170: string_proto_concat(i64, i64, i32, i32) -> i64

    let string_proto_concat_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, _env: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
        let mut result = get_string_value(&mut caller, this_val);
        let parts: Vec<String> = (0..args_count as u32).map(|i| {
            let arg = read_shadow_arg(&mut caller, args_base, i);
            get_string_value(&mut caller, arg)
        }).collect();
        for p in parts { result.push_str(&p); }
        store_runtime_string(&caller, result)
    });
    // Import 171: string_ends_with(i64, i64, i64) -> i64

    let string_ends_with_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, end_pos: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let search_str = get_string_value(&mut caller, search);
        let len = utf16_len(&s);
        let end_utf16 = if end_pos == value::encode_undefined() { len } else { (to_f64_or(end_pos, 0.0) as usize).min(len) };
        let end_byte = utf16_index_to_byte_offset(&s, end_utf16);
        value::encode_bool(if search_str.is_empty() { true } else { s[..end_byte].ends_with(&search_str) })
    });
    // Import 172: string_includes(i64, i64, i64) -> i64

    let string_includes_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, pos: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let start_utf16 = if pos == value::encode_undefined() { 0 } else { (to_f64_or(pos, 0.0) as usize).min(utf16_len(&s)) };
        let start_byte = utf16_index_to_byte_offset(&s, start_utf16);
        value::encode_bool(s[start_byte..].contains(&get_string_value(&mut caller, search)))
    });
    // Import 173: string_index_of(i64, i64, i64) -> i64

    let string_index_of_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, pos: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let search_str = get_string_value(&mut caller, search);
        let start_utf16 = if pos == value::encode_undefined() { 0 } else { (to_f64_or(pos, 0.0) as usize).min(utf16_len(&s)) };
        let start_byte = utf16_index_to_byte_offset(&s, start_utf16);
        match if search_str.is_empty() { Some(start_byte) } else { s[start_byte..].find(&search_str).map(|i| start_byte + i) } {
            Some(byte_idx) => value::encode_f64(byte_offset_to_utf16_index(&s, byte_idx) as f64),
            None => value::encode_f64(-1.0),
        }
    });
    // Import 174: string_last_index_of(i64, i64, i64) -> i64

    let string_last_index_of_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, pos: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let search_str = get_string_value(&mut caller, search);
        let len_utf16 = utf16_len(&s);
        if search_str.is_empty() {
            let end_utf16 = if pos == value::encode_undefined() { len_utf16 } else { (to_f64_or(pos, 0.0) as usize).min(len_utf16) };
            return value::encode_f64(end_utf16 as f64);
        }
        let end_utf16 = if pos == value::encode_undefined() { len_utf16 } else { (to_f64_or(pos, 0.0) as usize).min(len_utf16) };
        let end_byte = utf16_index_to_byte_offset(&s, end_utf16);
        match s[..end_byte].rfind(&search_str) {
            Some(byte_idx) => value::encode_f64(byte_offset_to_utf16_index(&s, byte_idx) as f64),
            None => value::encode_f64(-1.0),
        }
    });
    // Import 175: string_match_all(i64, i64, i32, i32) -> i64

    let string_match_all_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, _env: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            if args_count < 1 {
                *caller.data().runtime_error.lock().expect("runtime error mutex") =
                    Some("TypeError: String.prototype.matchAll requires a regexp argument".to_string());
                return value::encode_undefined();
            }
            let regexp = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_regexp(regexp) {
                *caller.data().runtime_error.lock().expect("runtime error mutex") =
                    Some("TypeError: String.prototype.matchAll called with non-RegExp".to_string());
                return value::encode_undefined();
            }
            let handle = value::decode_regexp_handle(regexp);
            let is_global = {
                let table = caller.data().regex_table.lock().unwrap();
                match table.get(handle as usize) {
                    Some(e) => e.flags.contains('g'),
                    None => {
                        *caller.data().runtime_error.lock().expect("runtime error mutex") =
                            Some("TypeError: String.prototype.matchAll called with a non-global RegExp".to_string());
                        return value::encode_undefined();
                    }
                }
            };
            if !is_global {
                *caller.data().runtime_error.lock().expect("runtime error mutex") =
                    Some("TypeError: String.prototype.matchAll called with a non-global RegExp".to_string());
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
                let group_count = m.captures.len() + 1;
                let arr = alloc_array(&mut caller, group_count as u32);
                let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                    continue;
                };
                for i in 0..group_count {
                    let elem = if let Some(range) = m.group(i) {
                        let group_str = &s[range];
                        store_runtime_string(&caller, group_str.to_string())
                    } else {
                        value::encode_undefined()
                    };
                    write_array_elem(&mut caller, arr_ptr, i as u32, elem);
                }
                write_array_length(&mut caller, arr_ptr, group_count as u32);
                let match_start = m.group(0).map(|r| r.start).unwrap_or(0);
                let index_val = value::encode_f64(match_start as f64);
                let input_val = store_runtime_string(&caller, s.clone());
                let _ = define_host_data_property_from_caller(&mut caller, arr_ptr as i64, "index", index_val);
                let _ = define_host_data_property_from_caller(&mut caller, arr_ptr as i64, "input", input_val);
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
    // Import 176: string_pad_end(i64, i64, i64) -> i64

    let string_pad_end_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, target_len: i64, pad_str_val: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let len = utf16_len(&s);
        let target = if value::is_f64(target_len) { to_f64_or(target_len, 0.0) as usize } else { 0 };
        if target <= len { return store_runtime_string(&caller, s); }
        let pad_str = if pad_str_val == value::encode_undefined() { " ".to_string() } else {
            let p = get_string_value(&mut caller, pad_str_val); if p.is_empty() { " ".to_string() } else { p }
        };
        let pad_chars: Vec<char> = pad_str.chars().collect();
        let mut result = s.clone();
        for i in len..target { result.push(pad_chars[(i - len) % pad_chars.len()]); }
        store_runtime_string(&caller, result)
    });
    // Import 177: string_pad_start(i64, i64, i64) -> i64

    let string_pad_start_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, target_len: i64, pad_str_val: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let len = utf16_len(&s);
        let target = if value::is_f64(target_len) { to_f64_or(target_len, 0.0) as usize } else { 0 };
        if target <= len { return store_runtime_string(&caller, s); }
        let pad_str = if pad_str_val == value::encode_undefined() { " ".to_string() } else {
            let p = get_string_value(&mut caller, pad_str_val); if p.is_empty() { " ".to_string() } else { p }
        };
        let pad_chars: Vec<char> = pad_str.chars().collect();
        let mut result = String::new();
        for i in 0..(target - len) { result.push(pad_chars[i % pad_chars.len()]); }
        result.push_str(&s);
        store_runtime_string(&caller, result)
    });
    // Import 178: string_repeat(i64, i64) -> i64

    let string_repeat_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, count: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let c = to_f64_or(count, 0.0);
        if c < 0.0 || c.is_infinite() {
            *caller.data().runtime_error.lock().expect("runtime error mutex") = Some("RangeError: Invalid count value".to_string());
            return value::encode_undefined();
        }
        store_runtime_string(&caller, s.repeat(c as usize))
    });
    // Import 179: string_replace_all(i64, i64, i64) -> i64

    let string_replace_all_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, replace: i64| -> i64 {
        if value::is_regexp(search) {
            *caller.data().runtime_error.lock().expect("runtime error mutex") = Some("TypeError: String.prototype.replaceAll called with a non-global RegExp argument".to_string());
            return value::encode_undefined();
        }
        let s = get_string_value(&mut caller, receiver);
        let search_str = get_string_value(&mut caller, search);
        let replace_str = get_string_value(&mut caller, replace);
        store_runtime_string(&caller, s.replace(&search_str, &replace_str))
    });
    // Import 180: string_slice(i64, i64, i64) -> i64

    let string_slice_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, start: i64, end: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let len = utf16_len(&s) as i64;
        let si = if value::is_f64(start) { let v = to_f64_or(start, 0.0) as i64; if v < 0 { (v + len).max(0) } else { v.min(len) } } else { 0 };
        let ei = if end == value::encode_undefined() { len } else if value::is_f64(end) { let v = to_f64_or(end, 0.0) as i64; if v < 0 { (v + len).max(0) } else { v.min(len) } } else { 0 };
        if si >= ei { return store_runtime_string(&caller, String::new()); }
        let start_byte = utf16_index_to_byte_offset(&s, si as usize);
        let end_byte = utf16_index_to_byte_offset(&s, ei as usize);
        store_runtime_string(&caller, s[start_byte..end_byte].to_string())
    });
    // Import 181: string_starts_with(i64, i64, i64) -> i64

    let string_starts_with_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, pos: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let start_utf16 = if pos == value::encode_undefined() { 0 } else { (to_f64_or(pos, 0.0) as usize).min(utf16_len(&s)) };
        let start_byte = utf16_index_to_byte_offset(&s, start_utf16);
        value::encode_bool(s[start_byte..].starts_with(&get_string_value(&mut caller, search)))
    });
    // Import 182: string_substring(i64, i64, i64) -> i64

    let string_substring_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, start: i64, end: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let len = utf16_len(&s) as i64;
        let s1 = if value::is_f64(start) { (to_f64_or(start, 0.0) as i64).max(0).min(len) } else { 0 };
        let e1 = if end == value::encode_undefined() { len } else { (to_f64_or(end, 0.0) as i64).max(0).min(len) };
        let (lo, hi) = if s1 < e1 { (s1, e1) } else { (e1, s1) };
        if lo >= hi { return store_runtime_string(&caller, String::new()); }
        let lo_byte = utf16_index_to_byte_offset(&s, lo as usize);
        let hi_byte = utf16_index_to_byte_offset(&s, hi as usize);
        store_runtime_string(&caller, s[lo_byte..hi_byte].to_string())
    });
    // Import 183: string_to_lower_case(i64) -> i64

    let string_to_lower_case_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver).to_lowercase();
        store_runtime_string(&caller, s)
    });
    // Import 184: string_to_upper_case(i64) -> i64

    let string_to_upper_case_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver).to_uppercase();
        store_runtime_string(&caller, s)
    });
    // Import 185: string_trim(i64) -> i64

    let string_trim_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver).trim().to_string();
        store_runtime_string(&caller, s)
    });
    // Import 186: string_trim_end(i64) -> i64

    let string_trim_end_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver).trim_end().to_string();
        store_runtime_string(&caller, s)
    });
    // Import 187: string_trim_start(i64) -> i64

    let string_trim_start_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver).trim_start().to_string();
        store_runtime_string(&caller, s)
    });
    // Import 188: string_to_string(i64) -> i64

    let string_to_string_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        if value::is_string(receiver) {
            let s = get_string_value(&mut caller, receiver);
            store_runtime_string(&caller, s)
        } else {
            obj_proto_to_string_impl(&mut caller, receiver)
        }
    });
    // Import 189: string_value_of(i64) -> i64

    let string_value_of_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        store_runtime_string(&caller, s)
    });
    // Import 190: string_iterator(i64) -> i64

    let string_iterator_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let s = get_string_value(&mut caller, receiver);
        let mut iters = caller.data().iterators.lock().expect("iterators mutex");
        let handle = iters.len() as u32;
        iters.push(IteratorState::StringIter {
            data: s.into_bytes(),
            byte_pos: 0,
        });
        value::encode_handle(value::TAG_ITERATOR, handle)
    });
    // Import 191: string_from_char_code(i64, i32, i32) -> i64

    let string_from_char_code_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, _env: i64, _this: i64, args_base: i32, args_count: i32| -> i64 {
        let mut result = String::new();
        for i in 0..args_count as u32 {
            let arg = read_shadow_arg(&mut caller, args_base, i);
            let code = to_uint16(arg) as u32;
            if let Some(c) = char::from_u32(code) { result.push(c); }
        }
        store_runtime_string(&caller, result)
    });
    // Import 192: string_from_code_point(i64, i32, i32) -> i64

    let string_from_code_point_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, _env: i64, _this: i64, args_base: i32, args_count: i32| -> i64 {
        let mut result = String::new();
        for i in 0..args_count as u32 {
            let arg = read_shadow_arg(&mut caller, args_base, i);
            let code = to_uint32(arg);
            if let Some(c) = char::from_u32(code) { result.push(c); }
        }
        store_runtime_string(&caller, result)
    });

    // ── Import 116: promise_create() -> i64 ────────────────────────────────

    vec![
        (16, string_concat),
        (17, string_concat_va),
        (166, string_at_fn),
        (167, string_char_at_fn),
        (168, string_char_code_at_fn),
        (169, string_code_point_at_fn),
        (170, string_proto_concat_fn),
        (171, string_ends_with_fn),
        (172, string_includes_fn),
        (173, string_index_of_fn),
        (174, string_last_index_of_fn),
        (175, string_match_all_fn),
        (176, string_pad_end_fn),
        (177, string_pad_start_fn),
        (178, string_repeat_fn),
        (179, string_replace_all_fn),
        (180, string_slice_fn),
        (181, string_starts_with_fn),
        (182, string_substring_fn),
        (183, string_to_lower_case_fn),
        (184, string_to_upper_case_fn),
        (185, string_trim_fn),
        (186, string_trim_end_fn),
        (187, string_trim_start_fn),
        (188, string_to_string_fn),
        (189, string_value_of_fn),
        (190, string_iterator_fn),
        (191, string_from_char_code_fn),
        (192, string_from_code_point_fn),
    ]
}
