//! String.prototype.replace / match / search / split / matchAll 再入方法。
//!
//! 替换回调经 `call_js`；`@@replace` 等 well-known 方法经 `call_symbol_method`。

use wjsm_host::{ExecContext, RuntimeString, Value};
use wjsm_ir::{value, wk_symbol};

/// 处理 JavaScript 替换模式 `$&` / `$n` / `$<name>` 等。
pub fn process_replacement_from_captures(
    replace_str: &str,
    s: &str,
    match_start: usize,
    match_end: usize,
    captures: &[Option<std::ops::Range<usize>>],
    named: &[(String, Option<std::ops::Range<usize>>)],
) -> String {
    let mut result = String::new();
    let chars: Vec<char> = replace_str.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            let next = chars[i + 1];
            match next {
                '$' => {
                    result.push('$');
                    i += 2;
                }
                '&' => {
                    result.push_str(&s[match_start..match_end]);
                    i += 2;
                }
                '`' => {
                    result.push_str(&s[..match_start]);
                    i += 2;
                }
                '\'' => {
                    result.push_str(&s[match_end..]);
                    i += 2;
                }
                '<' => {
                    if let Some(close_pos) = chars[i + 2..].iter().position(|&c| c == '>') {
                        let name: String = chars[i + 2..i + 2 + close_pos].iter().collect();
                        if let Some((_, range)) = named.iter().find(|(n, _)| n == &name)
                            && let Some(r) = range
                        {
                            result.push_str(&s[r.clone()]);
                        }
                        i += 3 + close_pos;
                    } else {
                        result.push('$');
                        result.push('<');
                        i += 2;
                    }
                }
                '0'..='9' => {
                    let mut group_num = (next as u8 - b'0') as usize;
                    let mut consumed = 2;
                    if group_num == 0 {
                        result.push('$');
                        result.push('0');
                        i += 2;
                        continue;
                    }
                    if i + 2 < chars.len()
                        && let Some('0'..='9') = chars.get(i + 2)
                    {
                        let next_digit = (chars[i + 2] as u8 - b'0') as usize;
                        let two_digit = group_num * 10 + next_digit;
                        if two_digit > 0 && two_digit <= captures.len() {
                            group_num = two_digit;
                            consumed = 3;
                        }
                    }
                    if group_num <= captures.len() {
                        if let Some(Some(range)) = captures.get(group_num) {
                            result.push_str(&s[range.clone()]);
                        }
                    } else {
                        result.push('$');
                        result.push(next);
                    }
                    i += consumed;
                }
                _ => {
                    result.push('$');
                    result.push(next);
                    i += 2;
                }
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

fn replace_callback_result_to_string<E: ExecContext>(ctx: &mut E, result: Value) -> String {
    if value::is_undefined(result) {
        return String::new();
    }
    if value::is_runtime_string_handle(result) || value::is_string(result) {
        return ctx.read_string_utf8_lossy(result);
    }
    ctx.value_to_display_string(result)
}

fn call_replace_func<E: ExecContext>(
    ctx: &mut E,
    func: Value,
    s: &str,
    match_start: usize,
    match_end: usize,
    captures: &[Option<std::ops::Range<usize>>],
    named_groups_obj: Value,
) -> String {
    let capture_count = captures.len().saturating_sub(1);
    let mut args = Vec::with_capacity(1 + capture_count + 3);
    args.push(ctx.store_string(&s[match_start..match_end]));
    for i in 1..=capture_count {
        let capture_val = if let Some(Some(range)) = captures.get(i) {
            ctx.store_string(&s[range.clone()])
        } else {
            value::encode_undefined()
        };
        args.push(capture_val);
    }
    args.push(value::encode_f64(match_start as f64));
    args.push(ctx.store_string(s));
    args.push(named_groups_obj);

    let result = ctx
        .call_js(func, value::encode_undefined(), &args)
        .unwrap_or_else(|_| value::encode_undefined());
    replace_callback_result_to_string(ctx, result)
}

fn build_groups_obj<E: ExecContext>(
    ctx: &mut E,
    named: &[(String, Option<std::ops::Range<usize>>)],
    s: &str,
) -> Value {
    if named.is_empty() {
        return value::encode_undefined();
    }
    let obj = ctx.alloc_null_proto_object(named.len() as u32);
    for (name, range) in named {
        let val = match range {
            Some(r) => ctx.store_string(&s[r.clone()]),
            None => value::encode_undefined(),
        };
        ctx.define_data_property(obj, name, val);
    }
    obj
}

/// 默认 replace 体（无 @@replace 时）。
pub fn string_replace_default<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    search: Value,
    replace: Value,
) -> Value {
    let s = ctx.get_runtime_string(receiver);
    let subject_lossy = s.to_utf8_lossy();
    let is_func_replace = value::is_callable(replace);

    if value::is_regexp(search) {
        let is_global = ctx.regexp_is_global(search);
        let matches = ctx.regexp_collect_matches(search, &subject_lossy, is_global);
        if matches.is_empty() {
            return ctx.store_runtime_string(s);
        }
        if is_global {
            let mut result = String::new();
            let mut last_end = 0;
            for mi in &matches {
                result.push_str(&subject_lossy[last_end..mi.start]);
                let replaced = if is_func_replace {
                    let groups_obj = build_groups_obj(ctx, &mi.named, &subject_lossy);
                    call_replace_func(
                        ctx,
                        replace,
                        &subject_lossy,
                        mi.start,
                        mi.end,
                        &mi.captures,
                        groups_obj,
                    )
                } else {
                    let replace_str_lossy = ctx.read_string_utf8_lossy(replace);
                    process_replacement_from_captures(
                        &replace_str_lossy,
                        &subject_lossy,
                        mi.start,
                        mi.end,
                        &mi.captures,
                        &mi.named,
                    )
                };
                result.push_str(&replaced);
                last_end = mi.end;
            }
            result.push_str(&subject_lossy[last_end..]);
            return ctx.store_runtime_string(RuntimeString::from_utf8_str(&result));
        }
        // 单次
        let mi = &matches[0];
        let groups_obj = build_groups_obj(ctx, &mi.named, &subject_lossy);
        let replaced = if is_func_replace {
            call_replace_func(
                ctx,
                replace,
                &subject_lossy,
                mi.start,
                mi.end,
                &mi.captures,
                groups_obj,
            )
        } else {
            let replace_str_lossy = ctx.read_string_utf8_lossy(replace);
            process_replacement_from_captures(
                &replace_str_lossy,
                &subject_lossy,
                mi.start,
                mi.end,
                &mi.captures,
                &mi.named,
            )
        };
        let mut result = String::new();
        result.push_str(&subject_lossy[..mi.start]);
        result.push_str(&replaced);
        result.push_str(&subject_lossy[mi.end..]);
        return ctx.store_runtime_string(RuntimeString::from_utf8_str(&result));
    }

    // 字符串搜索
    let search_str = ctx.get_runtime_string(search);
    if let Some(pos) = s.find_units(&search_str, 0) {
        let replaced = if is_func_replace {
            let search_lossy = search_str.to_utf8_lossy();
            let Some(byte_pos) = subject_lossy.find(&search_lossy) else {
                return ctx.store_runtime_string(s);
            };
            let captures = vec![Some(byte_pos..byte_pos + search_lossy.len())];
            RuntimeString::from_utf8_str(&call_replace_func(
                ctx,
                replace,
                &subject_lossy,
                byte_pos,
                byte_pos + search_lossy.len(),
                &captures,
                value::encode_undefined(),
            ))
        } else {
            ctx.get_runtime_string(replace)
        };
        let mut result = s.slice_units(0..pos);
        result.push_units_from(&replaced);
        result.push_units_from(&s.slice_units(pos + search_str.utf16_len()..s.utf16_len()));
        ctx.store_runtime_string(result)
    } else {
        ctx.store_runtime_string(s)
    }
}

pub fn string_replace<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    search: Value,
    replace: Value,
) -> Value {
    if let Some(result) =
        ctx.call_symbol_method(search, wk_symbol::REPLACE, search, &[receiver, replace])
    {
        return result;
    }
    string_replace_default(ctx, receiver, search, replace)
}

pub fn string_match<E: ExecContext>(ctx: &mut E, receiver: Value, regexp: Value) -> Value {
    if let Some(result) = ctx.call_symbol_method(regexp, wk_symbol::MATCH, regexp, &[receiver]) {
        return result;
    }
    ctx.regexp_string_match_default(receiver, regexp)
}

pub fn string_search<E: ExecContext>(ctx: &mut E, receiver: Value, regexp: Value) -> Value {
    if let Some(result) = ctx.call_symbol_method(regexp, wk_symbol::SEARCH, regexp, &[receiver]) {
        return result;
    }
    ctx.regexp_string_search_default(receiver, regexp)
}

pub fn string_split<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    sep: Value,
    limit: Value,
) -> Value {
    if let Some(result) = ctx.call_symbol_method(sep, wk_symbol::SPLIT, sep, &[receiver, limit]) {
        return result;
    }
    ctx.regexp_string_split_default(receiver, sep, limit)
}

pub fn string_match_all<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if args_count < 1 {
        ctx.set_last_error(
            "TypeError: String.prototype.matchAll requires a regexp argument".to_string(),
        );
        return value::encode_undefined();
    }
    let regexp = ctx.read_call_arg(
        wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
        0,
    );
    if let Some(result) = ctx.call_symbol_method(regexp, wk_symbol::MATCH_ALL, regexp, &[this_val])
    {
        return result;
    }
    ctx.regexp_match_all_default(this_val, regexp)
}
