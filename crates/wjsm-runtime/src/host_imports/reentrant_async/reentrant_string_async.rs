use super::*;
fn build_groups_obj_from_named(
    caller: &mut Caller<'_, RuntimeState>,
    named: &[(String, Option<std::ops::Range<usize>>)],
    s: &str,
) -> i64 {
    if named.is_empty() {
        return value::encode_undefined();
    }
    let obj = {
        let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_null_proto_object(caller, &_wjsm_env, named.len() as u32)
    };
    for (name, range) in named {
        let val = match range {
            Some(r) => store_runtime_string(caller, s[r.clone()].to_string()),
            None => value::encode_undefined(),
        };
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
    }
    obj
}

/// 处理 JavaScript 替换模式（Send-safe — 不持有 regress::Match 引用）
fn process_replacement_from_captures(
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
                        // 命名组不存在或未匹配 → 空字符串（ES 规范）
                        i += 3 + close_pos; // skip past $<name>
                    } else {
                        // 未闭合的 $<，保持原样
                        result.push('$');
                        result.push('<');
                        i += 2;
                    }
                }
                '0'..='9' => {
                    // $n or $nn → captured group
                    let mut group_num = (next as u8 - b'0') as usize;
                    let mut consumed = 2;
                    // ECMAScript: $0 不是特殊模式，应保持字面量
                    if group_num == 0 {
                        result.push('$');
                        result.push('0');
                        i += 2;
                        continue;
                    }
                    // 检查是否为两位数 $nn
                    if i + 2 < chars.len()
                        && let Some('0'..='9') = chars.get(i + 2)
                    {
                        let next_digit = (chars[i + 2] as u8 - b'0') as usize;
                        let two_digit = group_num * 10 + next_digit;
                        // $00 不是特殊模式，只有 $01-$99 是
                        if two_digit > 0 && two_digit <= captures.len() {
                            group_num = two_digit;
                            consumed = 3;
                        }
                    }
                    // 获取捕获组（group_num ≥ 1）
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

fn replace_callback_result_to_string(caller: &mut Caller<'_, RuntimeState>, result: i64) -> String {
    if value::is_undefined(result) {
        return String::new();
    }
    if value::is_runtime_string_handle(result) || value::is_string(result) {
        return get_string_value(caller, result);
    }
    eval_to_string(caller, result)
}

/// Async version of call_replace_func — uses shared Wasm callback shadow-stack handling.
async fn call_replace_func_async(
    caller: &mut Caller<'_, RuntimeState>,
    func: i64,
    s: &str,
    match_start: usize,
    match_end: usize,
    captures: &[Option<std::ops::Range<usize>>],
    named_groups_obj: i64,
) -> String {
    let capture_count = captures.len().saturating_sub(1);
    let mut args = Vec::with_capacity(1 + capture_count + 3);

    args.push(store_runtime_string(
        &*caller,
        s[match_start..match_end].to_string(),
    ));
    for i in 1..=capture_count {
        let capture_val = if let Some(Some(range)) = captures.get(i) {
            store_runtime_string(&*caller, s[range.clone()].to_string())
        } else {
            value::encode_undefined()
        };
        args.push(capture_val);
    }
    args.push(value::encode_f64(match_start as f64));
    args.push(store_runtime_string(&*caller, s.to_string()));
    args.push(named_groups_obj);

    let result = call_wasm_callback_async(caller, func, value::encode_undefined(), &args)
        .await
        .unwrap_or_else(|_| value::encode_undefined());
    replace_callback_result_to_string(caller, result)
}

pub(crate) async fn string_replace_async_body(
    mut caller: Caller<'_, RuntimeState>,
    receiver: i64,
    search: i64,
    replace: i64,
) -> i64 {
    let s = get_string_value(&mut caller, receiver);

    // 检查 replace 是否为函数（支持函数替换）
    let is_func_replace = value::is_callable(replace);

    if value::is_regexp(search) {
        let entry = {
            let table = caller.data().regex_table.lock().unwrap();
            match table.get(value::decode_regexp_handle(search) as usize) {
                Some(e) => e.clone(),
                None => return store_runtime_string(&caller, s),
            }
        };

        let is_global = entry.flags.contains('g');
        if is_global {
            // 全局替换：先收集所有匹配数据（避免 find_iter / Match 跨 await 导致非 Send）
            struct MatchInfo {
                start: usize,
                end: usize,
                captures: Vec<Option<std::ops::Range<usize>>>,
                named: Vec<(String, Option<std::ops::Range<usize>>)>,
            }
            let matches: Vec<MatchInfo> = entry
                .compiled
                .find_iter(&s)
                .map(|m| MatchInfo {
                    start: m.start(),
                    end: m.end(),
                    captures: (0..m.captures.len() + 1).map(|i| m.group(i)).collect(),
                    named: m
                        .named_groups()
                        .map(|(name, range)| (name.to_string(), range))
                        .collect(),
                })
                .collect();

            let mut result = String::new();
            let mut last_end = 0;
            for mi in &matches {
                // 添加匹配前的部分
                result.push_str(&s[last_end..mi.start]);
                // 根据是否为函数选择替换方式
                let replaced = if is_func_replace {
                    let groups_obj = if mi.named.is_empty() {
                        value::encode_undefined()
                    } else {
                        build_groups_obj_from_named(&mut caller, &mi.named, &s)
                    };
                    call_replace_func_async(
                        &mut caller,
                        replace,
                        &s,
                        mi.start,
                        mi.end,
                        &mi.captures,
                        groups_obj,
                    )
                    .await
                } else {
                    let replace_str = get_string_value(&mut caller, replace);
                    process_replacement_from_captures(
                        &replace_str,
                        &s,
                        mi.start,
                        mi.end,
                        &mi.captures,
                        &mi.named,
                    )
                };
                result.push_str(&replaced);
                last_end = mi.end;
            }
            result.push_str(&s[last_end..]);
            store_runtime_string(&caller, result)
        } else {
            // 单次替换
            match entry.compiled.find(&s) {
                Some(m) => {
                    let captures: Vec<Option<std::ops::Range<usize>>> =
                        (0..m.captures.len() + 1).map(|i| m.group(i)).collect();
                    let match_start = m.start();
                    let match_end = m.end();
                    let named: Vec<(String, Option<std::ops::Range<usize>>)> = m
                        .named_groups()
                        .map(|(name, range)| (name.to_string(), range))
                        .collect();
                    let groups_obj = if named.is_empty() {
                        value::encode_undefined()
                    } else {
                        build_groups_obj_from_named(&mut caller, &named, &s)
                    };
                    // Match 不再使用 — 所有数据已提取
                    let replaced = if is_func_replace {
                        call_replace_func_async(
                            &mut caller,
                            replace,
                            &s,
                            match_start,
                            match_end,
                            &captures,
                            groups_obj,
                        )
                        .await
                    } else {
                        let replace_str = get_string_value(&mut caller, replace);
                        process_replacement_from_captures(
                            &replace_str,
                            &s,
                            match_start,
                            match_end,
                            &captures,
                            &named,
                        )
                    };
                    let mut result = String::new();
                    result.push_str(&s[..match_start]);
                    result.push_str(&replaced);
                    result.push_str(&s[match_end..]);
                    store_runtime_string(&caller, result)
                }
                None => store_runtime_string(&caller, s),
            }
        }
    } else {
        // 字符串替换
        let search_str = get_string_value(&mut caller, search);
        if let Some(pos) = s.find(&search_str) {
            // 对于字符串搜索，函数替换的参数是：matched, offset, string
            let replaced = if is_func_replace {
                // 构造 captures（只有完整匹配）
                let captures = vec![Some(pos..pos + search_str.len())];
                call_replace_func_async(
                    &mut caller,
                    replace,
                    &s,
                    pos,
                    pos + search_str.len(),
                    &captures,
                    value::encode_undefined(),
                )
                .await
            } else {
                get_string_value(&mut caller, replace)
            };
            let mut result = String::new();
            result.push_str(&s[..pos]);
            result.push_str(&replaced);
            result.push_str(&s[pos + search_str.len()..]);
            store_runtime_string(&caller, result)
        } else {
            store_runtime_string(&caller, s)
        }
    }
}

pub(crate) fn define_primitive_core_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "string_replace",
        |caller: Caller<'_, RuntimeState>, (receiver, search, replace): (i64, i64, i64)| {
            Box::new(
                async move { string_replace_async_body(caller, receiver, search, replace).await },
            )
        },
    )?;
    Ok(())
}
