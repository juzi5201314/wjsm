use wasmtime::*;
use wjsm_ir::value;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let regex_create_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>,
         pat_ptr: i32,
         pat_len: i32,
         flags_ptr: i32,
         flags_len: i32|
         -> i64 {
            // 从 WASM 内存读取 pattern 和 flags
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return value::encode_undefined();
            };
            let data = memory.data(&caller);

            let pat_start = pat_ptr as usize;
            let pat_end = (pat_ptr as usize).saturating_add(pat_len as usize);
            if pat_end > data.len() {
                return value::encode_undefined();
            }
            let pat_bytes = &data[pat_start..pat_end];
            // 去掉 nul terminator
            let pattern = String::from_utf8_lossy(if pat_bytes.ends_with(&[0]) {
                &pat_bytes[..pat_bytes.len() - 1]
            } else {
                pat_bytes
            })
            .into_owned();

            let flags_start = flags_ptr as usize;
            let flags_end = (flags_ptr as usize).saturating_add(flags_len as usize);
            if flags_end > data.len() {
                return value::encode_undefined();
            }
            let flags_bytes = &data[flags_start..flags_end];
            let flags = String::from_utf8_lossy(if flags_bytes.ends_with(&[0]) {
                &flags_bytes[..flags_bytes.len() - 1]
            } else {
                flags_bytes
            })
            .into_owned();

            // 编译正则表达式
            match regress::Regex::with_flags(&pattern, flags.as_str()) {
                Ok(compiled) => {
                    let mut table = caller.data_mut().regex_table.lock().unwrap();
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
                    // 编译失败，抛出 SyntaxError
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime error mutex") =
                        Some(format!("SyntaxError: Invalid regular expression: {}", e));
                    value::encode_undefined()
                }
            }
        },
    );

    // ── Import 110: regex_test(i64, i64) → i64 ───────────────────────────────────

    let regex_test_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, regex_val: i64, str_val: i64| -> i64 {
            if !value::is_regexp(regex_val) {
                return value::encode_bool(false);
            }
            let handle = value::decode_regexp_handle(regex_val);

            // 获取字符串内容
            let s = get_string_value(&mut caller, str_val);

            // 单次锁定获取正则信息：is_global、is_sticky、start_pos、entry clone
            let (entry, is_global, is_sticky, start_pos) = {
                let table = caller.data().regex_table.lock().unwrap();
                match table.get(handle as usize) {
                    Some(e) => {
                        let is_global = e.flags.contains('g');
                        let is_sticky = e.flags.contains('y');
                        // 全局或粘性模式从 lastIndex 开始，否则从 0 开始
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

            // 执行匹配：全局或粘性模式从 lastIndex 开始
            let match_result = if is_global || is_sticky {
                entry.compiled.find_from(&s, start_pos).next()
            } else {
                entry.compiled.find(&s)
            };

            // 粘性模式：匹配必须从 start_pos 位置精确开始
            // 如果匹配从更后面的位置开始，视为失败
            let (found, match_end) = match match_result {
                Some(ref m) if is_sticky && m.start() != start_pos => {
                    // 粘性模式匹配失败：匹配位置不在 lastIndex
                    (false, None)
                }
                Some(m) => (true, Some(m.end())),
                None => (false, None),
            };

            // 更新 lastIndex（全局或粘性模式）
            if is_global || is_sticky {
                let mut table = caller.data().regex_table.lock().unwrap();
                if let Some(e) = table.get_mut(handle as usize) {
                    if let Some(end) = match_end {
                        // 找到匹配：更新 lastIndex 到匹配结束位置
                        e.last_index = end as i64;
                    } else {
                        // 无匹配或粘性失败：重置 lastIndex 为 0
                        e.last_index = 0;
                    }
                }
            }

            value::encode_bool(found)
        },
    );

    // ── Import 111: regex_exec(i64, i64) → i64 ───────────────────────────────────

    let regex_exec_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, regex_val: i64, str_val: i64| -> i64 {
            if !value::is_regexp(regex_val) {
                return value::encode_null();
            }
            let handle = value::decode_regexp_handle(regex_val);

            // 获取字符串内容
            let s = get_string_value(&mut caller, str_val);

            // 单次锁定获取正则信息：is_global、lastIndex、entry clone
            // 单次锁定获取正则信息：is_global、is_sticky、lastIndex、entry clone
            let (entry, is_global, is_sticky, start_pos) = {
                let table = caller.data().regex_table.lock().unwrap();
                match table.get(handle as usize) {
                    Some(e) => {
                        let is_global = e.flags.contains('g');
                        let is_sticky = e.flags.contains('y');
                        // 全局或粘性模式从 lastIndex 开始，否则从 0 开始
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

            // 执行匹配（无锁）
            match entry.compiled.find_from(&s, start_pos).next() {
                // 粘性模式：匹配必须从 start_pos 位置精确开始
                Some(ref m) if is_sticky && m.start() != start_pos => {
                    // 粘性模式匹配失败：匹配位置不在 lastIndex
                    if is_global || is_sticky {
                        let mut table = caller.data().regex_table.lock().unwrap();
                        if let Some(e) = table.get_mut(handle as usize) {
                            e.last_index = 0;
                        }
                    }
                    value::encode_null()
                }
                Some(m) => {
                    // 更新 lastIndex（全局或粘性模式）
                    if is_global || is_sticky {
                        let mut table = caller.data().regex_table.lock().unwrap();
                        if let Some(e) = table.get_mut(handle as usize) {
                            e.last_index = m.end() as i64;
                        }
                    }

                    // 构建结果数组 [full_match, group1, group2, ...]
                    let group_count = m.captures.len() + 1; // +1 for group 0 (full match)
                    let arr = alloc_array(&mut caller, group_count as u32);
                    let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                        return value::encode_null();
                    };

                    for i in 0..group_count {
                        let elem = if let Some(range) = m.group(i) {
                            let group_str = &s[range];
                            store_runtime_string(&caller, group_str.to_string())
                        } else {
                            value::encode_undefined() // 捕获组未匹配时为 undefined
                        };
                        write_array_elem(&mut caller, arr_ptr, i as u32, elem);
                    }
                    write_array_length(&mut caller, arr_ptr, group_count as u32);
                    arr
                }
                None => {
                    // 无匹配时重置 lastIndex
                    if is_global || is_sticky {
                        let mut table = caller.data().regex_table.lock().unwrap();
                        if let Some(e) = table.get_mut(handle as usize) {
                            e.last_index = 0;
                        }
                    }
                    value::encode_null()
                }
            }
        },
    );

    // ── Import 112: string_match(i64, i64) → i64 ─────────────────────────────────

    let string_match_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, regexp: i64| -> i64 {
            // str.match(regexp)
            let s = get_string_value(&mut caller, receiver);

            if !value::is_regexp(regexp) {
                // 如果不是 RegExp，根据 ECMAScript 规范，将其转换为 RegExp
                // 相当于 new RegExp(regexp)
                let pattern = get_string_value(&mut caller, regexp);
                match regress::Regex::with_flags(&pattern, "") {
                    Ok(compiled) => {
                        let mut table = caller.data_mut().regex_table.lock().unwrap();
                        let new_handle = table.len() as u32;
                        table.push(RegexEntry {
                            pattern: pattern.clone(),
                            flags: String::new(),
                            compiled,
                            last_index: 0,
                        });
                        // 继续使用新创建的 RegExp 进行匹配
                        let entry = table.get(new_handle as usize).unwrap().clone();
                        drop(table);

                        // 非全局匹配：返回第一个匹配结果
                        match entry.compiled.find(&s) {
                            Some(m) => {
                                let group_count = m.captures.len() + 1;
                                let arr = alloc_array(&mut caller, group_count as u32);
                                let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                                    return value::encode_null();
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
                                return arr;
                            }
                            None => return value::encode_null(),
                        }
                    }
                    Err(e) => {
                        // 创建 RegExp 失败，抛出 SyntaxError
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .expect("runtime error mutex") =
                            Some(format!("SyntaxError: Invalid regular expression: {}", e));
                        return value::encode_null();
                    }
                }
            }

            let handle = value::decode_regexp_handle(regexp);
            let (entry, is_global) = {
                let mut table = caller.data().regex_table.lock().unwrap();
                let entry = match table.get_mut(handle as usize) {
                    Some(e) => e,
                    None => return value::encode_null(),
                };
                let is_global = entry.flags.contains('g');
                // 全局匹配时重置 lastIndex
                if is_global {
                    entry.last_index = 0;
                }
                (entry.clone(), is_global)
            };

            if is_global {
                // 返回所有匹配的数组
                let mut matches = Vec::new();
                for m in entry.compiled.find_iter(&s) {
                    if let Some(range) = m.group(0) {
                        matches.push(s[range].to_string());
                    }
                }
                // 创建数组并返回
                if matches.is_empty() {
                    value::encode_null()
                } else {
                    let arr = alloc_array(&mut caller, matches.len() as u32);
                    let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                        return value::encode_null();
                    };
                    for (i, m) in matches.iter().enumerate() {
                        let elem = store_runtime_string(&caller, m.clone());
                        write_array_elem(&mut caller, arr_ptr, i as u32, elem);
                    }
                    write_array_length(&mut caller, arr_ptr, matches.len() as u32);
                    arr
                }
            } else {
                // 非全局：返回 exec 结果（数组或 null）
                match entry.compiled.find(&s) {
                    Some(m) => {
                        // 构建结果数组 [full_match, group1, group2, ...]
                        let group_count = m.captures.len() + 1; // +1 for group 0 (full match)
                        let arr = alloc_array(&mut caller, group_count as u32);
                        let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                            return value::encode_null();
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
                        arr
                    }
                    None => value::encode_null(),
                }
            }
        },
    );

    // ── Import 113: string_replace(i64, i64, i64) → i64 ──────────────────────────

    let string_replace_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, replace: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);

            // 检查 replace 是否为函数（支持函数替换）
            let is_func_replace = value::is_callable(replace);

            /// 处理 JavaScript 替换模式：$$, $&, $`, $', $n, $nn
            fn process_replacement(
                replace_str: &str,
                s: &str,
                match_start: usize,
                match_end: usize,
                captures: &[Option<std::ops::Range<usize>>],
            ) -> String {
                let mut result = String::new();
                let chars: Vec<char> = replace_str.chars().collect();
                let mut i = 0;
                while i < chars.len() {
                    if chars[i] == '$' && i + 1 < chars.len() {
                        let next = chars[i + 1];
                        match next {
                            '$' => {
                                // $$ → $
                                result.push('$');
                                i += 2;
                            }
                            '&' => {
                                // $& → matched substring
                                result.push_str(&s[match_start..match_end]);
                                i += 2;
                            }
                            '`' => {
                                // $` → portion before match
                                result.push_str(&s[..match_start]);
                                i += 2;
                            }
                            '\'' => {
                                // $' → portion after match
                                result.push_str(&s[match_end..]);
                                i += 2;
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
                                if i + 2 < chars.len() {
                                    if let Some('0'..='9') = chars.get(i + 2) {
                                        let next_digit = (chars[i + 2] as u8 - b'0') as usize;
                                        let two_digit = group_num * 10 + next_digit;
                                        // $00 不是特殊模式，只有 $01-$99 是
                                        if two_digit > 0 && two_digit <= captures.len() {
                                            group_num = two_digit;
                                            consumed = 3;
                                        }
                                    }
                                }
                                // 获取捕获组
                                if group_num < captures.len() {
                                    if let Some(ref range) = captures[group_num] {
                                        result.push_str(&s[range.clone()]);
                                    }
                                    // 如果捕获组未匹配，什么都不添加
                                } else {
                                    // 无效的组号，保持原样
                                    result.push('$');
                                    result.push(next);
                                }
                                i += consumed;
                            }
                            _ => {
                                // 未知模式，保持原样
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

            /// 调用替换函数并返回替换字符串
            fn call_replace_func(
                caller: &mut Caller<'_, RuntimeState>,
                func: i64,
                s: &str,
                match_start: usize,
                match_end: usize,
                captures: &[Option<std::ops::Range<usize>>],
            ) -> String {
                // 参数数量：matched + captures + offset + string
                let capture_count = captures.len().saturating_sub(1); // 不包括 group 0（完整匹配）
                let args_count = 1 + capture_count + 1 + 1; // matched + captures + offset + string

                // 获取 shadow_sp 和 memory
                let shadow_sp_global = caller
                    .get_export("__shadow_sp")
                    .and_then(|e| e.into_global())
                    .unwrap();
                let shadow_sp = shadow_sp_global.get(&mut *caller).i32().unwrap();
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .unwrap();

                // 写入参数到 shadow stack
                let mut arg_idx = 0;

                // 1. matched substring
                let matched_val =
                    store_runtime_string(&*caller, s[match_start..match_end].to_string());
                memory
                    .write(
                        &mut *caller,
                        (shadow_sp + arg_idx * 8) as usize,
                        &matched_val.to_le_bytes(),
                    )
                    .unwrap();
                arg_idx += 1;

                // 2. capture groups (从 group 1 开始)
                for i in 1..=capture_count {
                    let capture_val = if let Some(Some(range)) = captures.get(i) {
                        store_runtime_string(&*caller, s[range.clone()].to_string())
                    } else {
                        value::encode_undefined()
                    };
                    memory
                        .write(
                            &mut *caller,
                            (shadow_sp + arg_idx * 8) as usize,
                            &capture_val.to_le_bytes(),
                        )
                        .unwrap();
                    arg_idx += 1;
                }

                // 3. offset
                let offset_val = value::encode_f64(match_start as f64);
                memory
                    .write(
                        &mut *caller,
                        (shadow_sp + arg_idx * 8) as usize,
                        &offset_val.to_le_bytes(),
                    )
                    .unwrap();
                arg_idx += 1;

                // 4. original string
                let string_val = store_runtime_string(&*caller, s.to_string());
                memory
                    .write(
                        &mut *caller,
                        (shadow_sp + arg_idx * 8) as usize,
                        &string_val.to_le_bytes(),
                    )
                    .unwrap();

                // 调用函数
                let result = resolve_and_call(
                    caller,
                    func,
                    value::encode_undefined(),
                    0,
                    args_count as i32,
                );

                // 将返回值转换为字符串
                get_string_value(caller, result)
            }

            if value::is_regexp(search) {
                let handle = value::decode_regexp_handle(search);
                let table = caller.data().regex_table.lock().unwrap();
                let entry = match table.get(handle as usize) {
                    Some(e) => e.clone(),
                    None => return store_runtime_string(&caller, s),
                };
                drop(table);

                let is_global = entry.flags.contains('g');
                if is_global {
                    // 全局替换：迭代所有匹配并替换
                    let mut result = String::new();
                    let mut last_end = 0;
                    for m in entry.compiled.find_iter(&s) {
                        // 添加匹配前的部分
                        result.push_str(&s[last_end..m.start()]);
                        // 收集捕获组
                        let captures: Vec<Option<std::ops::Range<usize>>> =
                            (0..m.captures.len() + 1).map(|i| m.group(i)).collect();
                        // 根据是否为函数选择替换方式
                        let replaced = if is_func_replace {
                            call_replace_func(
                                &mut caller,
                                replace,
                                &s,
                                m.start(),
                                m.end(),
                                &captures,
                            )
                        } else {
                            let replace_str = get_string_value(&mut caller, replace);
                            process_replacement(&replace_str, &s, m.start(), m.end(), &captures)
                        };
                        result.push_str(&replaced);
                        last_end = m.end();
                    }
                    result.push_str(&s[last_end..]);
                    store_runtime_string(&caller, result)
                } else {
                    // 单次替换
                    match entry.compiled.find(&s) {
                        Some(m) => {
                            let captures: Vec<Option<std::ops::Range<usize>>> =
                                (0..m.captures.len() + 1).map(|i| m.group(i)).collect();
                            let replaced = if is_func_replace {
                                call_replace_func(
                                    &mut caller,
                                    replace,
                                    &s,
                                    m.start(),
                                    m.end(),
                                    &captures,
                                )
                            } else {
                                let replace_str = get_string_value(&mut caller, replace);
                                process_replacement(&replace_str, &s, m.start(), m.end(), &captures)
                            };
                            let mut result = String::new();
                            result.push_str(&s[..m.start()]);
                            result.push_str(&replaced);
                            result.push_str(&s[m.end()..]);
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
                        call_replace_func(
                            &mut caller,
                            replace,
                            &s,
                            pos,
                            pos + search_str.len(),
                            &captures,
                        )
                    } else {
                        let replace_str = get_string_value(&mut caller, replace);
                        replace_str
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
        },
    );

    // ── Import 114: string_search(i64, i64) → i64 ────────────────────────────────

    let string_search_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, regexp: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);

            // 根据 ECMAScript 22.1.3.21，非 RegExp 参数应转换为 RegExp
            let handle = if value::is_regexp(regexp) {
                value::decode_regexp_handle(regexp)
            } else {
                // 将参数转换为字符串，然后创建 RegExp
                let pattern = get_string_value(&mut caller, regexp);
                // 使用空 flags 创建 RegExp
                match regress::Regex::with_flags(&pattern, "") {
                    Ok(compiled) => {
                        let mut table = caller.data().regex_table.lock().unwrap();
                        let handle = table.len() as u32;
                        table.push(RegexEntry {
                            pattern,
                            flags: String::new(),
                            compiled,
                            last_index: 0,
                        });
                        handle
                    }
                    Err(_) => {
                        // 正则编译失败，返回 -1（不抛出错误，因为原始值可能不是有效的正则模式）
                        return value::encode_f64(-1.0);
                    }
                }
            };

            let table = caller.data().regex_table.lock().unwrap();
            let entry = match table.get(handle as usize) {
                Some(e) => e.clone(),
                None => return value::encode_f64(-1.0),
            };
            drop(table);

            match entry.compiled.find(&s) {
                Some(m) => value::encode_f64(m.start() as f64),
                None => value::encode_f64(-1.0),
            }
        },
    );

    // ── Import 115: string_split(i64, i64, i64) → i64 ────────────────────────────

    let string_split_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, sep: i64, limit: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);

            // 解析 limit（遵循 ECMAScript ToUint32 语义）
            let limit_val = if value::is_undefined(limit) {
                usize::MAX // undefined 表示无限制
            } else if value::is_f64(limit) {
                let n = f64::from_bits(limit as u64);
                // ToUint32: NaN, Infinity → 0; 其他值应用模 2^32
                if n.is_nan() || n.is_infinite() {
                    0
                } else {
                    // 使用数学模运算，正确处理负数
                    // -1 mod 2^32 = 4294967295
                    let truncated = n.trunc();
                    let modulus = 4294967296.0_f64; // 2^32
                    let result = truncated - (truncated / modulus).floor() * modulus;
                    // result 在 [0, 2^32) 范围内
                    result as u32 as usize
                }
            } else {
                // 非数字类型，尝试转换为字符串再解析
                let s = get_string_value(&mut caller, limit);
                s.parse::<f64>()
                    .map(|n| {
                        if n.is_nan() || n.is_infinite() {
                            0usize
                        } else {
                            let truncated = n.trunc();
                            let modulus = 4294967296.0_f64;
                            let result = truncated - (truncated / modulus).floor() * modulus;
                            result as u32 as usize
                        }
                    })
                    .unwrap_or(0)
            };

            // limit 为 0 时返回空数组
            if limit_val == 0 {
                let arr = alloc_array(&mut caller, 0);
                let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                    return value::encode_null();
                };
                write_array_length(&mut caller, arr_ptr, 0);
                return arr;
            }

            if value::is_regexp(sep) {
                // 正则分割
                let handle = value::decode_regexp_handle(sep);
                let table = caller.data().regex_table.lock().unwrap();
                let entry = match table.get(handle as usize) {
                    Some(e) => e.clone(),
                    None => return value::encode_null(),
                };
                drop(table);

                // 使用 Vec<i64> 存储结果，支持字符串和 undefined
                let mut parts: Vec<i64> = Vec::new();
                let mut last_end = 0;
                for m in entry.compiled.find_iter(&s) {
                    if parts.len() >= limit_val {
                        break;
                    }
                    let start = m.start();
                    let end = m.end();
                    if start > last_end {
                        // 添加匹配前的文本部分
                        parts.push(store_runtime_string(
                            &caller,
                            s[last_end..start].to_string(),
                        ));
                    }
                    // 根据 ECMAScript 规范，将捕获组插入结果数组
                    // 捕获组从索引 1 开始（索引 0 是完整匹配）
                    for i in 1..m.captures.len() + 1 {
                        if parts.len() >= limit_val {
                            break;
                        }
                        let elem = if let Some(range) = m.group(i) {
                            store_runtime_string(&caller, s[range].to_string())
                        } else {
                            value::encode_undefined() // 捕获组未匹配时为 undefined
                        };
                        parts.push(elem);
                    }
                    last_end = end;
                }
                // 添加最后一部分
                if parts.len() < limit_val && last_end < s.len() {
                    parts.push(store_runtime_string(&caller, s[last_end..].to_string()));
                }

                // 创建数组并返回
                let arr = alloc_array(&mut caller, parts.len() as u32);
                let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                    return value::encode_null();
                };
                for (i, elem) in parts.iter().enumerate() {
                    write_array_elem(&mut caller, arr_ptr, i as u32, *elem);
                }
                write_array_length(&mut caller, arr_ptr, parts.len() as u32);
                arr
            } else {
                // 字符串分割
                let sep_str = get_string_value(&mut caller, sep);
                // 空字符串分隔符：返回每个字符的数组
                if sep_str.is_empty() {
                    let chars: Vec<String> =
                        s.chars().map(|c| c.to_string()).take(limit_val).collect();
                    let arr = alloc_array(&mut caller, chars.len() as u32);
                    let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                        return value::encode_null();
                    };
                    for (i, ch) in chars.iter().enumerate() {
                        let elem = store_runtime_string(&caller, ch.clone());
                        write_array_elem(&mut caller, arr_ptr, i as u32, elem);
                    }
                    write_array_length(&mut caller, arr_ptr, chars.len() as u32);
                    return arr;
                }
                let parts: Vec<&str> = s.split(&sep_str).take(limit_val).collect();
                // 创建数组并返回
                let arr = alloc_array(&mut caller, parts.len() as u32);
                let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                    return value::encode_null();
                };
                for (i, part) in parts.iter().enumerate() {
                    let elem = store_runtime_string(&caller, part.to_string());
                    write_array_elem(&mut caller, arr_ptr, i as u32, elem);
                }
                write_array_length(&mut caller, arr_ptr, parts.len() as u32);
                arr
            }
        },
    );

    vec![
        (109, regex_create_fn),
        (110, regex_test_fn),
        (111, regex_exec_fn),
        (112, string_match_fn),
        (113, string_replace_fn),
        (114, string_search_fn),
        (115, string_split_fn),
    ]
}
