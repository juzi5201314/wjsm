use std::ops::Range;

use wasmtime::Caller;
use wjsm_ir::{value, wk_symbol};

use crate::*;

pub(crate) const RE_METHOD_EXEC: u8 = 0;
pub(crate) const RE_METHOD_TEST: u8 = 1;
pub(crate) const RE_METHOD_TO_STRING: u8 = 2;
pub(crate) const RE_METHOD_SYMBOL_MATCH: u8 = 3;
pub(crate) const RE_METHOD_SYMBOL_REPLACE: u8 = 4;
pub(crate) const RE_METHOD_SYMBOL_SEARCH: u8 = 5;
pub(crate) const RE_METHOD_SYMBOL_SPLIT: u8 = 6;
pub(crate) const RE_METHOD_SYMBOL_MATCH_ALL: u8 = 7;

#[derive(Clone, Debug)]
pub(crate) struct RegExpStringMatchInfo {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) captures: Vec<Option<Range<usize>>>,
    pub(crate) named: Vec<(String, Option<Range<usize>>)>,
}

fn set_regexp_error(caller: &mut Caller<'_, RuntimeState>, message: String) {
    set_runtime_error(caller.data(), message);
}

fn js_to_string(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    if value::is_undefined(val) {
        return "undefined".to_string();
    }
    if value::is_string(val) || value::is_runtime_string_handle(val) {
        return get_string_value(caller, val);
    }
    render_value(caller, val).unwrap_or_default()
}

fn string_from_utf16_code_unit(unit: u16) -> String {
    if !(0xD800..=0xDFFF).contains(&unit)
        && let Some(ch) = char::from_u32(unit as u32)
    {
        return ch.to_string();
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

fn regexp_entry(caller: &mut Caller<'_, RuntimeState>, regexp: i64) -> Option<RegexEntry> {
    if !value::is_regexp(regexp) {
        return None;
    }
    let handle = value::decode_regexp_handle(regexp) as usize;
    caller
        .data()
        .regex_table
        .lock()
        .ok()
        .and_then(|table| table.get(handle).cloned())
}

fn pattern_from_arg(caller: &mut Caller<'_, RuntimeState>, pattern_arg: i64) -> String {
    if value::is_undefined(pattern_arg) {
        String::new()
    } else if let Some(entry) = regexp_entry(caller, pattern_arg) {
        entry.pattern
    } else {
        js_to_string(caller, pattern_arg)
    }
}

fn flags_from_arg(
    caller: &mut Caller<'_, RuntimeState>,
    pattern_arg: i64,
    flags_arg: i64,
) -> String {
    if value::is_undefined(flags_arg) {
        if let Some(entry) = regexp_entry(caller, pattern_arg) {
            entry.flags
        } else {
            String::new()
        }
    } else {
        js_to_string(caller, flags_arg)
    }
}

pub(crate) fn regexp_create_from_parts(
    caller: &mut Caller<'_, RuntimeState>,
    pattern: String,
    flags: String,
) -> i64 {
    const VALID_FLAGS: &[char] = &['d', 'g', 'i', 'm', 's', 'u', 'v', 'y'];
    let mut seen = [false; 128];
    for c in flags.chars() {
        if !VALID_FLAGS.contains(&c) {
            set_regexp_error(
                caller,
                format!("SyntaxError: Invalid regular expression flag: '{}'", c),
            );
            return value::encode_undefined();
        }
        let idx = c as usize;
        if idx < seen.len() {
            if seen[idx] {
                set_regexp_error(
                    caller,
                    format!("SyntaxError: Duplicate regular expression flag: '{}'", c),
                );
                return value::encode_undefined();
            }
            seen[idx] = true;
        }
    }

    if seen['u' as usize] && seen['v' as usize] {
        set_regexp_error(
            caller,
            "SyntaxError: Invalid regular expression flags: u and v cannot be combined".to_string(),
        );
        return value::encode_undefined();
    }

    let engine_flags: String = flags
        .chars()
        .filter(|c| matches!(c, 'i' | 'm' | 's' | 'u' | 'v'))
        .collect();

    match regress::Regex::with_flags(&pattern, engine_flags.as_str()) {
        Ok(compiled) => {
            let mut table = caller
                .data_mut()
                .regex_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let handle = table.len() as u32;
            table.push(RegexEntry {
                pattern,
                flags,
                compiled,
                last_index: 0,
            });
            value::encode_regexp_handle(handle)
        }
        Err(e) => {
            set_regexp_error(
                caller,
                format!("SyntaxError: Invalid regular expression: {}", e),
            );
            value::encode_undefined()
        }
    }
}

pub(crate) fn regexp_constructor_impl(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: &[i64],
) -> i64 {
    let pattern_arg = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let flags_arg = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let is_construct =
        value::is_object(this_val) || value::is_array(this_val) || value::is_proxy(this_val);

    if !is_construct && value::is_regexp(pattern_arg) && value::is_undefined(flags_arg) {
        return pattern_arg;
    }

    let pattern = pattern_from_arg(caller, pattern_arg);
    let flags = flags_from_arg(caller, pattern_arg, flags_arg);
    regexp_create_from_parts(caller, pattern, flags)
}

fn build_match_result_from_parts(
    caller: &mut Caller<'_, RuntimeState>,
    s: &str,
    flags: &str,
    info: &RegExpStringMatchInfo,
) -> i64 {
    let group_count = info.captures.len() as u32;
    let arr = alloc_array(caller, group_count);
    let Some(arr_ptr) = resolve_array_ptr(caller, arr) else {
        return value::encode_null();
    };

    for (i, capture) in info.captures.iter().enumerate() {
        let elem = match capture {
            Some(range) => store_runtime_string(caller, s[range.clone()].to_string()),
            None => value::encode_undefined(),
        };
        write_array_elem(caller, arr_ptr, i as u32, elem);
    }
    write_array_length(caller, arr_ptr, group_count);

    let index_val = value::encode_f64(byte_offset_to_utf16_index(s, info.start) as f64);
    let _ = define_host_data_property_from_caller(caller, arr, "index", index_val);
    let input_val = store_runtime_string(caller, s.to_string());
    let _ = define_host_data_property_from_caller(caller, arr, "input", input_val);

    if info.named.is_empty() {
        let _ =
            define_host_data_property_from_caller(caller, arr, "groups", value::encode_undefined());
    } else {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        let groups = alloc_host_null_proto_object(caller, &env, info.named.len() as u32);
        for (name, range) in &info.named {
            let val = match range {
                Some(r) => store_runtime_string(caller, s[r.clone()].to_string()),
                None => value::encode_undefined(),
            };
            let _ = define_host_data_property_from_caller(caller, groups, name, val);
        }
        let _ = define_host_data_property_from_caller(caller, arr, "groups", groups);
    }

    if flags.contains('d') {
        let indices_arr = alloc_array(caller, group_count);
        let Some(indices_ptr) = resolve_array_ptr(caller, indices_arr) else {
            return value::encode_null();
        };
        for (i, capture) in info.captures.iter().enumerate() {
            let elem = match capture {
                Some(range) => {
                    let pair = alloc_array(caller, 2);
                    let pair_ptr = resolve_array_ptr(caller, pair).unwrap_or(0);
                    write_array_elem(
                        caller,
                        pair_ptr,
                        0,
                        value::encode_f64(byte_offset_to_utf16_index(s, range.start) as f64),
                    );
                    write_array_elem(
                        caller,
                        pair_ptr,
                        1,
                        value::encode_f64(byte_offset_to_utf16_index(s, range.end) as f64),
                    );
                    write_array_length(caller, pair_ptr, 2);
                    pair
                }
                None => value::encode_undefined(),
            };
            write_array_elem(caller, indices_ptr, i as u32, elem);
        }
        write_array_length(caller, indices_ptr, group_count);
        // 构建 indices.groups（与上方 arr.groups 对应，但值为 [start, end] 数组）
        if info.named.is_empty() {
            let _ = define_host_data_property_from_caller(
                caller,
                indices_arr,
                "groups",
                value::encode_undefined(),
            );
        } else {
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            let indices_groups =
                alloc_host_null_proto_object(caller, &env, info.named.len() as u32);
            for (name, range) in &info.named {
                let val = match range {
                    Some(r) => {
                        let pair = alloc_array(caller, 2);
                        let pair_ptr = resolve_array_ptr(caller, pair).unwrap_or(0);
                        write_array_elem(
                            caller,
                            pair_ptr,
                            0,
                            value::encode_f64(byte_offset_to_utf16_index(s, r.start) as f64),
                        );
                        write_array_elem(
                            caller,
                            pair_ptr,
                            1,
                            value::encode_f64(byte_offset_to_utf16_index(s, r.end) as f64),
                        );
                        write_array_length(caller, pair_ptr, 2);
                        pair
                    }
                    None => value::encode_undefined(),
                };
                let _ = define_host_data_property_from_caller(caller, indices_groups, name, val);
            }
            let _ = define_host_data_property_from_caller(
                caller,
                indices_arr,
                "groups",
                indices_groups,
            );
        }
        let _ = define_host_data_property_from_caller(caller, arr, "indices", indices_arr);
    }

    arr
}

fn match_info_from_match(m: &regress::Match) -> RegExpStringMatchInfo {
    RegExpStringMatchInfo {
        start: m.start(),
        end: m.end(),
        captures: (0..m.captures.len() + 1).map(|i| m.group(i)).collect(),
        named: m
            .named_groups()
            .map(|(name, range)| (name.to_string(), range))
            .collect(),
    }
}

fn advance_string_index(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return index.saturating_add(1);
    }
    s[index..]
        .chars()
        .next()
        .map(|ch| index + ch.len_utf8())
        .unwrap_or(index.saturating_add(1))
}

pub(crate) fn regexp_test_impl(
    caller: &mut Caller<'_, RuntimeState>,
    regex_val: i64,
    str_val: i64,
) -> i64 {
    if !value::is_regexp(regex_val) {
        return value::encode_bool(false);
    }
    let handle = value::decode_regexp_handle(regex_val);
    let s = get_string_value(caller, str_val);
    let (entry, is_global, is_sticky, start_pos) = {
        let table = caller
            .data()
            .regex_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match table.get(handle as usize) {
            Some(e) => {
                let is_global = e.flags.contains('g');
                let is_sticky = e.flags.contains('y');
                let start_pos = if is_global || is_sticky {
                    e.last_index as usize
                } else {
                    0
                };
                (e.clone(), is_global, is_sticky, start_pos)
            }
            None => return value::encode_bool(false),
        }
    };

    let match_result = if is_global || is_sticky {
        entry.compiled.find_from(&s, start_pos).next()
    } else {
        entry.compiled.find(&s)
    };
    let (found, match_end) = match match_result {
        Some(ref m) if is_sticky && m.start() != start_pos => (false, None),
        Some(m) => (true, Some(m.end())),
        None => (false, None),
    };

    if is_global || is_sticky {
        let mut table = caller
            .data()
            .regex_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(e) = table.get_mut(handle as usize) {
            if let Some(end) = match_end {
                let mut new_index = end as i64;
                if end == start_pos && start_pos < s.len() {
                    new_index = advance_string_index(&s, start_pos) as i64;
                }
                e.last_index = new_index;
            } else {
                e.last_index = 0;
            }
        }
    }

    value::encode_bool(found)
}

pub(crate) fn regexp_exec_impl(
    caller: &mut Caller<'_, RuntimeState>,
    regex_val: i64,
    str_val: i64,
) -> i64 {
    if !value::is_regexp(regex_val) {
        return value::encode_null();
    }
    let handle = value::decode_regexp_handle(regex_val);
    let s = get_string_value(caller, str_val);
    let (entry, is_global, is_sticky, start_pos) = {
        let table = caller
            .data()
            .regex_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match table.get(handle as usize) {
            Some(e) => {
                let is_global = e.flags.contains('g');
                let is_sticky = e.flags.contains('y');
                let start_pos = if is_global || is_sticky {
                    e.last_index as usize
                } else {
                    0
                };
                (e.clone(), is_global, is_sticky, start_pos)
            }
            None => return value::encode_null(),
        }
    };

    match entry.compiled.find_from(&s, start_pos).next() {
        Some(ref m) if is_sticky && m.start() != start_pos => {
            if is_global || is_sticky {
                let mut table = caller
                    .data()
                    .regex_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(e) = table.get_mut(handle as usize) {
                    e.last_index = 0;
                }
            }
            value::encode_null()
        }
        Some(m) => {
            if is_global || is_sticky {
                let mut table = caller
                    .data()
                    .regex_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(e) = table.get_mut(handle as usize) {
                    let end = m.end();
                    let mut new_index = end as i64;
                    if end == start_pos && start_pos < s.len() {
                        new_index = advance_string_index(&s, start_pos) as i64;
                    }
                    e.last_index = new_index;
                }
            }
            let info = match_info_from_match(&m);
            build_match_result_from_parts(caller, &s, &entry.flags, &info)
        }
        None => {
            if is_global || is_sticky {
                let mut table = caller
                    .data()
                    .regex_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(e) = table.get_mut(handle as usize) {
                    e.last_index = 0;
                }
            }
            value::encode_null()
        }
    }
}

pub(crate) fn regexp_string_match_default(
    caller: &mut Caller<'_, RuntimeState>,
    receiver: i64,
    regexp: i64,
) -> i64 {
    let s = get_string_value(caller, receiver);
    if !value::is_regexp(regexp) {
        let pattern = js_to_string(caller, regexp);
        return match regress::Regex::with_flags(&pattern, "") {
            Ok(compiled) => match compiled.find(&s) {
                Some(m) => {
                    let info = match_info_from_match(&m);
                    build_match_result_from_parts(caller, &s, "", &info)
                }
                None => value::encode_null(),
            },
            Err(e) => {
                set_regexp_error(
                    caller,
                    format!("SyntaxError: Invalid regular expression: {}", e),
                );
                value::encode_null()
            }
        };
    }

    let handle = value::decode_regexp_handle(regexp);
    let (entry, is_global) = {
        let mut table = caller
            .data()
            .regex_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = match table.get_mut(handle as usize) {
            Some(e) => e,
            None => return value::encode_null(),
        };
        let is_global = entry.flags.contains('g');
        if is_global {
            entry.last_index = 0;
        }
        (entry.clone(), is_global)
    };

    if is_global {
        let mut matches = Vec::new();
        for m in entry.compiled.find_iter(&s) {
            if let Some(range) = m.group(0) {
                matches.push(s[range].to_string());
            }
        }
        if matches.is_empty() {
            value::encode_null()
        } else {
            let arr = alloc_array(caller, matches.len() as u32);
            let Some(arr_ptr) = resolve_array_ptr(caller, arr) else {
                return value::encode_null();
            };
            for (i, m) in matches.iter().enumerate() {
                let elem = store_runtime_string(caller, m.clone());
                write_array_elem(caller, arr_ptr, i as u32, elem);
            }
            write_array_length(caller, arr_ptr, matches.len() as u32);
            arr
        }
    } else {
        let is_sticky = entry.flags.contains('y');
        let last_idx = entry.last_index as usize;
        let match_result = if is_sticky {
            entry.compiled.find_from(&s, last_idx).next()
        } else {
            entry.compiled.find(&s)
        };
        match match_result {
            Some(m) if !is_sticky || m.start() == last_idx => {
                let info = match_info_from_match(&m);
                build_match_result_from_parts(caller, &s, &entry.flags, &info)
            }
            _ => value::encode_null(),
        }
    }
}

pub(crate) fn regexp_string_search_default(
    caller: &mut Caller<'_, RuntimeState>,
    receiver: i64,
    regexp: i64,
) -> i64 {
    let s = get_string_value(caller, receiver);
    if !value::is_regexp(regexp) {
        let pattern = js_to_string(caller, regexp);
        return match regress::Regex::with_flags(&pattern, "") {
            Ok(compiled) => match compiled.find(&s) {
                Some(m) => value::encode_f64(byte_offset_to_utf16_index(&s, m.start()) as f64),
                None => value::encode_f64(-1.0),
            },
            Err(e) => {
                set_regexp_error(
                    caller,
                    format!("SyntaxError: Invalid regular expression: {}", e),
                );
                value::encode_undefined()
            }
        };
    }

    let handle = value::decode_regexp_handle(regexp);
    let (entry, prev_last_index) = {
        let mut table = caller
            .data()
            .regex_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = match table.get_mut(handle as usize) {
            Some(e) => e,
            None => return value::encode_f64(-1.0),
        };
        let prev = entry.last_index;
        if entry.flags.contains('g') || entry.flags.contains('y') {
            entry.last_index = 0;
        }
        (entry.clone(), prev)
    };
    let result = match entry.compiled.find(&s) {
        Some(m) => value::encode_f64(byte_offset_to_utf16_index(&s, m.start()) as f64),
        None => value::encode_f64(-1.0),
    };
    {
        let mut table = caller
            .data()
            .regex_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(e) = table.get_mut(handle as usize) {
            e.last_index = prev_last_index;
        }
    }
    result
}

pub(crate) fn regexp_string_split_default(
    caller: &mut Caller<'_, RuntimeState>,
    receiver: i64,
    sep: i64,
    limit: i64,
) -> i64 {
    let s = get_string_value(caller, receiver);
    let limit_val = if value::is_undefined(limit) {
        usize::MAX
    } else if value::is_f64(limit) {
        let n = value::decode_f64(limit);
        if n.is_nan() || n.is_infinite() {
            0
        } else {
            let truncated = n.trunc();
            let modulus = 4294967296.0_f64;
            (truncated - (truncated / modulus).floor() * modulus) as u32 as usize
        }
    } else {
        js_to_string(caller, limit)
            .parse::<f64>()
            .map(|n| {
                if n.is_nan() || n.is_infinite() {
                    0usize
                } else {
                    let truncated = n.trunc();
                    let modulus = 4294967296.0_f64;
                    (truncated - (truncated / modulus).floor() * modulus) as u32 as usize
                }
            })
            .unwrap_or(0)
    };

    if limit_val == 0 {
        let arr = alloc_array(caller, 0);
        if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
            write_array_length(caller, arr_ptr, 0);
        }
        return arr;
    }

    if value::is_regexp(sep) {
        let entry = match regexp_entry(caller, sep) {
            Some(e) => e,
            None => return value::encode_null(),
        };
        let mut parts = Vec::new();
        let mut last_end = 0;
        for m in entry.compiled.find_iter(&s) {
            if parts.len() >= limit_val {
                break;
            }
            let start = m.start();
            let end = m.end();
            if start >= last_end {
                parts.push(store_runtime_string(caller, s[last_end..start].to_string()));
            }
            for i in 1..m.captures.len() + 1 {
                if parts.len() >= limit_val {
                    break;
                }
                let elem = if let Some(range) = m.group(i) {
                    store_runtime_string(caller, s[range].to_string())
                } else {
                    value::encode_undefined()
                };
                parts.push(elem);
            }
            last_end = end;
        }
        if parts.len() < limit_val && last_end <= s.len() {
            parts.push(store_runtime_string(caller, s[last_end..].to_string()));
        }
        let arr = alloc_array(caller, parts.len() as u32);
        let Some(arr_ptr) = resolve_array_ptr(caller, arr) else {
            return value::encode_null();
        };
        for (i, elem) in parts.iter().enumerate() {
            write_array_elem(caller, arr_ptr, i as u32, *elem);
        }
        write_array_length(caller, arr_ptr, parts.len() as u32);
        arr
    } else {
        let sep_str = js_to_string(caller, sep);
        if sep_str.is_empty() {
            let chars: Vec<String> = s
                .encode_utf16()
                .map(string_from_utf16_code_unit)
                .take(limit_val)
                .collect();
            let arr = alloc_array(caller, chars.len() as u32);
            let Some(arr_ptr) = resolve_array_ptr(caller, arr) else {
                return value::encode_null();
            };
            for (i, ch) in chars.iter().enumerate() {
                let elem = store_runtime_string(caller, ch.clone());
                write_array_elem(caller, arr_ptr, i as u32, elem);
            }
            write_array_length(caller, arr_ptr, chars.len() as u32);
            return arr;
        }
        let parts: Vec<&str> = s.split(&sep_str).take(limit_val).collect();
        let arr = alloc_array(caller, parts.len() as u32);
        let Some(arr_ptr) = resolve_array_ptr(caller, arr) else {
            return value::encode_null();
        };
        for (i, part) in parts.iter().enumerate() {
            let elem = store_runtime_string(caller, part.to_string());
            write_array_elem(caller, arr_ptr, i as u32, elem);
        }
        write_array_length(caller, arr_ptr, parts.len() as u32);
        arr
    }
}

pub(crate) fn regexp_match_all_default(
    caller: &mut Caller<'_, RuntimeState>,
    receiver: i64,
    mut regexp: i64,
) -> i64 {
    if !value::is_regexp(regexp) {
        let pattern = js_to_string(caller, regexp);
        regexp = regexp_create_from_parts(caller, pattern, "g".to_string());
        if !value::is_regexp(regexp) {
            return value::encode_undefined();
        }
    }

    let s = get_string_value(caller, receiver);
    let entry = match regexp_entry(caller, regexp) {
        Some(e) => e,
        None => return value::encode_undefined(),
    };
    if !entry.flags.contains('g') {
        set_regexp_error(
            caller,
            "TypeError: String.prototype.matchAll called with a non-global RegExp".to_string(),
        );
        return value::encode_undefined();
    }

    let start_index = entry.last_index.max(0) as usize;
    let iter_handle = {
        let mut iters = caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = iters.len() as u32;
        iters.push(IteratorState::RegExpStringIter {
            entry,
            string: s,
            next_index: start_index,
            current: None,
            done: false,
        });
        handle
    };
    // ECMAScript §22.2.9.1 CreateRegExpStringIterator：返回带 %RegExpStringIteratorPrototype%
    // 形态的迭代器对象（next + [Symbol.iterator]→this），可被 for-of / 展开 / Array.from /
    // 直接 next() 消费。底层惰性状态存于 RegExpStringIter（按 handle 引用）。
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let iter_obj = alloc_host_object(caller, &env, 2);
    let next_val = create_native_callable(
        caller.data(),
        NativeCallable::RegExpStringIteratorNext { iter_handle },
    );
    let _ = define_host_data_property_from_caller(caller, iter_obj, "next", next_val);
    let self_val = create_native_callable(caller.data(), NativeCallable::RegExpStringIteratorSelf);
    let _ = define_host_data_property_by_name_id(
        caller,
        iter_obj,
        encode_symbol_name_id(wk_symbol::ITERATOR),
        self_val,
    );
    iter_obj
}

/// RegExp String Iterator 的 next()：推进底层状态并返回 IteratorResult `{value, done}`。
pub(crate) fn regexp_string_iterator_step(
    caller: &mut Caller<'_, RuntimeState>,
    iter_handle: u32,
) -> i64 {
    let done = regexp_string_iter_ensure_current(caller, iter_handle as usize);
    if done {
        return make_iter_result(caller, value::encode_undefined(), true);
    }
    let value = regexp_string_iter_value(caller, iter_handle as usize);
    regexp_string_iter_next(caller, iter_handle as usize);
    make_iter_result(caller, value, false)
}

fn make_iter_result(caller: &mut Caller<'_, RuntimeState>, value: i64, done: bool) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let _ = define_host_data_property_from_caller(caller, obj, "value", value);
    let _ = define_host_data_property_from_caller(caller, obj, "done", value::encode_bool(done));
    obj
}

pub(crate) fn regexp_string_iter_ensure_current(
    caller: &mut Caller<'_, RuntimeState>,
    handle_idx: usize,
) -> bool {
    let mut iters = caller
        .data()
        .iterators
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(IteratorState::RegExpStringIter {
        entry,
        string,
        next_index,
        current,
        done,
    }) = iters.get_mut(handle_idx)
    else {
        return true;
    };
    if *done {
        return true;
    }
    if current.is_some() {
        return false;
    }
    match entry.compiled.find_from(string, *next_index).next() {
        Some(m) => {
            *current = Some(match_info_from_match(&m));
            false
        }
        None => {
            *done = true;
            true
        }
    }
}

pub(crate) fn regexp_string_iter_value(
    caller: &mut Caller<'_, RuntimeState>,
    handle_idx: usize,
) -> i64 {
    let (entry_flags, string, current) = {
        let mut iters = caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(IteratorState::RegExpStringIter {
            entry,
            string,
            current,
            ..
        }) = iters.get_mut(handle_idx)
        else {
            return value::encode_undefined();
        };
        let Some(info) = current.clone() else {
            return value::encode_undefined();
        };
        (entry.flags.clone(), string.clone(), info)
    };
    build_match_result_from_parts(caller, &string, &entry_flags, &current)
}

pub(crate) fn regexp_string_iter_next(caller: &mut Caller<'_, RuntimeState>, handle_idx: usize) {
    let mut iters = caller
        .data()
        .iterators
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(IteratorState::RegExpStringIter {
        string,
        next_index,
        current,
        done,
        ..
    }) = iters.get_mut(handle_idx)
    else {
        return;
    };
    if let Some(info) = current.take() {
        *next_index = if info.end == info.start {
            advance_string_index(string, info.end)
        } else {
            info.end
        };
    } else if !*done {
        *done = true;
    }
}

fn sorted_flags(flags: &str) -> String {
    ['d', 'g', 'i', 'm', 's', 'u', 'v', 'y']
        .into_iter()
        .filter(|flag| flags.contains(*flag))
        .collect()
}

pub(crate) fn primitive_regexp_get_property_impl(
    caller: &mut Caller<'_, RuntimeState>,
    boxed: i64,
    name_id: u32,
) -> i64 {
    if !value::is_regexp(boxed) {
        return value::encode_undefined();
    }

    if is_symbol_name_id(name_id) {
        let (_, symbol_idx) = decode_name_id(name_id);
        return match symbol_idx {
            wk_symbol::MATCH => create_native_callable(
                caller.data(),
                NativeCallable::RegExpPrimitiveMethod {
                    method: RE_METHOD_SYMBOL_MATCH,
                },
            ),
            wk_symbol::REPLACE => create_native_callable(
                caller.data(),
                NativeCallable::RegExpPrimitiveMethod {
                    method: RE_METHOD_SYMBOL_REPLACE,
                },
            ),
            wk_symbol::SEARCH => create_native_callable(
                caller.data(),
                NativeCallable::RegExpPrimitiveMethod {
                    method: RE_METHOD_SYMBOL_SEARCH,
                },
            ),
            wk_symbol::SPLIT => create_native_callable(
                caller.data(),
                NativeCallable::RegExpPrimitiveMethod {
                    method: RE_METHOD_SYMBOL_SPLIT,
                },
            ),
            wk_symbol::MATCH_ALL => create_native_callable(
                caller.data(),
                NativeCallable::RegExpPrimitiveMethod {
                    method: RE_METHOD_SYMBOL_MATCH_ALL,
                },
            ),
            wk_symbol::TO_STRING_TAG => store_runtime_string(caller, "RegExp".to_string()),
            _ => value::encode_undefined(),
        };
    }

    let entry = match regexp_entry(caller, boxed) {
        Some(e) => e,
        None => return value::encode_undefined(),
    };
    match read_string_bytes(caller, name_id).as_slice() {
        b"exec" => create_native_callable(
            caller.data(),
            NativeCallable::RegExpPrimitiveMethod {
                method: RE_METHOD_EXEC,
            },
        ),
        b"test" => create_native_callable(
            caller.data(),
            NativeCallable::RegExpPrimitiveMethod {
                method: RE_METHOD_TEST,
            },
        ),
        b"toString" => create_native_callable(
            caller.data(),
            NativeCallable::RegExpPrimitiveMethod {
                method: RE_METHOD_TO_STRING,
            },
        ),
        b"lastIndex" => value::encode_f64(entry.last_index as f64),
        b"source" => store_runtime_string(
            caller,
            if entry.pattern.is_empty() {
                "(?:)".to_string()
            } else {
                entry.pattern
            },
        ),
        b"flags" => store_runtime_string(caller, sorted_flags(&entry.flags)),
        b"global" => value::encode_bool(entry.flags.contains('g')),
        b"ignoreCase" => value::encode_bool(entry.flags.contains('i')),
        b"multiline" => value::encode_bool(entry.flags.contains('m')),
        b"dotAll" => value::encode_bool(entry.flags.contains('s')),
        b"unicode" => value::encode_bool(entry.flags.contains('u')),
        b"sticky" => value::encode_bool(entry.flags.contains('y')),
        b"hasIndices" => value::encode_bool(entry.flags.contains('d')),
        _ => value::encode_undefined(),
    }
}

pub(crate) fn primitive_regexp_set_property_impl(
    caller: &mut Caller<'_, RuntimeState>,
    boxed: i64,
    name_id: u32,
    val: i64,
) {
    if !value::is_regexp(boxed) || is_symbol_name_id(name_id) {
        return;
    }
    if read_string_bytes(caller, name_id).as_slice() != b"lastIndex" {
        return;
    }
    let num = value_to_number_or_exception(caller, val);
    if value::is_exception(num) {
        return;
    }
    let n = value::decode_f64(num);
    let new_index = if !n.is_finite() || n <= 0.0 {
        0
    } else if n >= i64::MAX as f64 {
        i64::MAX
    } else {
        n.trunc() as i64
    };
    let handle = value::decode_regexp_handle(boxed) as usize;
    let mut table = caller
        .data()
        .regex_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = table.get_mut(handle) {
        entry.last_index = new_index;
    }
}

pub(crate) async fn call_symbol_method_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    symbol_idx: u32,
    this_arg: i64,
    args: &[i64],
) -> Option<i64> {
    let name_id = encode_symbol_name_id(symbol_idx);
    let method = match get_method_by_name_id(caller, target, name_id) {
        Ok(Some(method)) => method,
        Ok(None) => return None,
        Err(exc) => return Some(exc),
    };
    if value::is_native_callable(method) {
        return Some(
            call_native_callable_with_args_from_caller_async(
                caller,
                method,
                this_arg,
                args.to_vec(),
            )
            .await
            .unwrap_or_else(value::encode_undefined),
        );
    }
    if value::is_callable(method) {
        return Some(
            call_wasm_callback_async(caller, method, this_arg, args)
                .await
                .unwrap_or_else(|_| value::encode_undefined()),
        );
    }
    Some(value::encode_undefined())
}

pub(crate) fn invoke_regexp_primitive_method(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    method: u8,
    args: &[i64],
) -> i64 {
    match method {
        RE_METHOD_EXEC => regexp_exec_impl(
            caller,
            this_val,
            args.first()
                .copied()
                .unwrap_or_else(value::encode_undefined),
        ),
        RE_METHOD_TEST => regexp_test_impl(
            caller,
            this_val,
            args.first()
                .copied()
                .unwrap_or_else(value::encode_undefined),
        ),
        RE_METHOD_TO_STRING => {
            let Some(entry) = regexp_entry(caller, this_val) else {
                set_regexp_error(
                    caller,
                    "TypeError: RegExp.prototype.toString called on incompatible receiver"
                        .to_string(),
                );
                return value::encode_undefined();
            };
            store_runtime_string(
                caller,
                format!(
                    "/{}/{}",
                    entry.pattern.replace('/', "\\/"),
                    sorted_flags(&entry.flags)
                ),
            )
        }
        RE_METHOD_SYMBOL_MATCH => regexp_string_match_default(
            caller,
            args.first()
                .copied()
                .unwrap_or_else(value::encode_undefined),
            this_val,
        ),
        RE_METHOD_SYMBOL_SEARCH => regexp_string_search_default(
            caller,
            args.first()
                .copied()
                .unwrap_or_else(value::encode_undefined),
            this_val,
        ),
        RE_METHOD_SYMBOL_SPLIT => regexp_string_split_default(
            caller,
            args.first()
                .copied()
                .unwrap_or_else(value::encode_undefined),
            this_val,
            args.get(1).copied().unwrap_or_else(value::encode_undefined),
        ),
        RE_METHOD_SYMBOL_MATCH_ALL => regexp_match_all_default(
            caller,
            args.first()
                .copied()
                .unwrap_or_else(value::encode_undefined),
            this_val,
        ),
        RE_METHOD_SYMBOL_REPLACE => {
            // @@replace 可能 reentrant 回用户 JS（替换函数），sync 路径无法安全 reentry。
            // wasm 发起的调用一律经 invoke_regexp_primitive_method_async（native_call 为 async）。
            // sync 路径已退役，仅可能由宿主内部非 reentrant 场景误入，返回可捕获错误。
            set_regexp_error(
                caller,
                "TypeError: RegExp.prototype[Symbol.replace] unsupported on sync path".to_string(),
            );
            value::encode_undefined()
        }
        _ => value::encode_undefined(),
    }
}

pub(crate) async fn invoke_regexp_primitive_method_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    method: u8,
    args: &[i64],
) -> i64 {
    if method == RE_METHOD_SYMBOL_REPLACE {
        return crate::string_replace_default_async_body(
            caller,
            args.first()
                .copied()
                .unwrap_or_else(value::encode_undefined),
            this_val,
            args.get(1).copied().unwrap_or_else(value::encode_undefined),
        )
        .await;
    }
    invoke_regexp_primitive_method(caller, this_val, method, args)
}
