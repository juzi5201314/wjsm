use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::*;

pub(crate) fn define_primitive_core(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let bigint_from_literal_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, ptr: i32, _len: i32| -> i64 {
            let s = read_string(&mut caller, ptr as u32).unwrap_or_default();
            // 去掉末尾可能存在的 nul 字符
            let trimmed = s.trim_end_matches('\0');
            if let Ok(bigint) = trimmed.parse::<num_bigint::BigInt>() {
                let mut table = caller
                    .data()
                    .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                let handle = table.len() as u32;
                table.push(bigint);
                value::encode_bigint_handle(handle)
            } else {
                value::encode_undefined()
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "bigint_from_literal",
        bigint_from_literal_fn,
    )?;

    // ── BigInt arithmetic helpers ─────────────────────────────────────
    fn bigint_binary_op(
        caller: &mut Caller<'_, RuntimeState>,
        a: i64,
        b: i64,
        op: impl Fn(&num_bigint::BigInt, &num_bigint::BigInt) -> num_bigint::BigInt,
    ) -> i64 {
        let a_handle = value::decode_bigint_handle(a) as usize;
        let b_handle = value::decode_bigint_handle(b) as usize;
        let (a_val, b_val) = {
            let table = caller
                .data()
                .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
            (table.get(a_handle).cloned(), table.get(b_handle).cloned())
        };
        match (a_val, b_val) {
            (Some(av), Some(bv)) => {
                let result = op(&av, &bv);
                let mut table = caller
                    .data()
                    .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                let handle = table.len() as u32;
                table.push(result);
                value::encode_bigint_handle(handle)
            }
            _ => value::encode_undefined(),
        }
    }

    let bigint_add_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            bigint_binary_op(&mut caller, a, b, |x, y| x + y)
        },
    );
    linker.define(&mut store, "env", "bigint_add", bigint_add_fn)?;
    let bigint_sub_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            bigint_binary_op(&mut caller, a, b, |x, y| x - y)
        },
    );
    linker.define(&mut store, "env", "bigint_sub", bigint_sub_fn)?;
    let bigint_mul_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            bigint_binary_op(&mut caller, a, b, |x, y| x * y)
        },
    );
    linker.define(&mut store, "env", "bigint_mul", bigint_mul_fn)?;
    let bigint_div_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let (av, bv) = {
                let table = caller
                    .data()
                    .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                (table.get(a_handle).cloned(), table.get(b_handle).cloned())
            };
            match (av, bv) {
                (Some(x), Some(y)) => {
                    if y == 0u32.into() {
                        *caller
                            .data()
                            .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some("RangeError: BigInt division by zero".to_string());
                        return value::encode_undefined();
                    }
                    let result = x / y;
                    let mut table = caller
                        .data()
                        .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                    let handle = table.len() as u32;
                    table.push(result);
                    value::encode_bigint_handle(handle)
                }
                _ => value::encode_undefined(),
            }
        },
    );
    linker.define(&mut store, "env", "bigint_div", bigint_div_fn)?;
    let bigint_mod_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let (av, bv) = {
                let table = caller
                    .data()
                    .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                (table.get(a_handle).cloned(), table.get(b_handle).cloned())
            };
            match (av, bv) {
                (Some(x), Some(y)) => {
                    if y == 0u32.into() {
                        *caller
                            .data()
                            .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some("RangeError: BigInt division by zero".to_string());
                        return value::encode_undefined();
                    }
                    let result = x % y;
                    let mut table = caller
                        .data()
                        .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                    let handle = table.len() as u32;
                    table.push(result);
                    value::encode_bigint_handle(handle)
                }
                _ => value::encode_undefined(),
            }
        },
    );
    linker.define(&mut store, "env", "bigint_mod", bigint_mod_fn)?;
    let bigint_pow_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let (av, bv) = {
                let table = caller
                    .data()
                    .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                (table.get(a_handle).cloned(), table.get(b_handle).cloned())
            };
            match (av, bv) {
                (Some(x), Some(y)) => {
                    // Per spec §6.1.6.1.9: negative exponent throws RangeError
                    if y.sign() == num_bigint::Sign::Minus {
                        *caller
                            .data()
                            .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some("RangeError: BigInt exponent must be non-negative".to_string());
                        return value::encode_undefined();
                    }
                    let exp = match y.to_u32() {
                        Some(e) => e,
                        None => {
                            *caller
                                .data()
                                .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some("RangeError: BigInt exponent too large".to_string());
                            return value::encode_undefined();
                        }
                    };
                    let result = x.pow(exp);
                    let mut table = caller
                        .data()
                        .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                    let handle = table.len() as u32;
                    table.push(result);
                    value::encode_bigint_handle(handle)
                }
                _ => value::encode_undefined(),
            }
        },
    );
    linker.define(&mut store, "env", "bigint_pow", bigint_pow_fn)?;
    let bigint_neg_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, a: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let a_val = {
                let table = caller
                    .data()
                    .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                table.get(a_handle).cloned()
            };
            if let Some(av) = a_val {
                let result = -av;
                let mut table = caller
                    .data()
                    .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                let handle = table.len() as u32;
                table.push(result);
                value::encode_bigint_handle(handle)
            } else {
                value::encode_undefined()
            }
        },
    );
    linker.define(&mut store, "env", "bigint_neg", bigint_neg_fn)?;
    let bigint_eq_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let eq = {
                let table = caller
                    .data()
                    .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                table
                    .get(a_handle)
                    .zip(table.get(b_handle))
                    .map(|(x, y)| x == y)
                    .unwrap_or(false)
            };
            value::encode_bool(eq)
        },
    );
    linker.define(&mut store, "env", "bigint_eq", bigint_eq_fn)?;
    let bigint_cmp_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let cmp = {
                let table = caller
                    .data()
                    .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                match (table.get(a_handle), table.get(b_handle)) {
                    (Some(x), Some(y)) => {
                        use std::cmp::Ordering;
                        match x.cmp(y) {
                            Ordering::Less => -1.0f64,
                            Ordering::Equal => 0.0f64,
                            Ordering::Greater => 1.0f64,
                        }
                    }
                    _ => f64::NAN,
                }
            };
            cmp.to_bits() as i64
        },
    );
    linker.define(&mut store, "env", "bigint_cmp", bigint_cmp_fn)?;

    // ═══════════════════════════════════════════════════════════════════
    // ── Symbol host functions ──────────────────────────────────────────
    // ═══════════════════════════════════════════════════════════════════

    // ── Import 105: symbol_create(i64) → i64 ──────────────────────────
    let symbol_create_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, desc: i64| -> i64 {
            let description = if value::is_undefined(desc) {
                None
            } else if value::is_string(desc) {
                // 字符串直接使用原始值，不裁剪引号（ES §20.4.1.1）
                Some(get_string_value(&mut caller, desc))
            } else {
                Some(render_value(&mut caller, desc).unwrap_or_default())
            };
            let mut table = caller
                .data()
                .symbol_table.lock().unwrap_or_else(|e| e.into_inner());
            let handle = table.len() as u32;
            table.push(SymbolEntry {
                description,
                global_key: None,
            });
            value::encode_symbol_handle(handle)
        },
    );
    linker.define(&mut store, "env", "symbol_create", symbol_create_fn)?;

    // ── Import 106: symbol_for(i64) → i64 ─────────────────────────────
    // 全局 symbol 注册表（static 变量，与 RuntimeState 生命周期相同）
    // Symbol.for(key) 返回全局注册表中的 symbol
    let symbol_for_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, key: i64| -> i64 {
            let key_str = match json_parse_to_string(&mut caller, key) {
                Ok(s) => s,
                Err(exception) => return exception,
            };
            let mut table = caller
                .data()
                .symbol_table.lock().unwrap_or_else(|e| e.into_inner());
            // 查找是否已有同 key 的 symbol
            for (idx, entry) in table.iter().enumerate() {
                if entry.global_key.as_deref() == Some(&key_str) {
                    return value::encode_symbol_handle(idx as u32);
                }
            }
            // 创建新 symbol
            let handle = table.len() as u32;
            table.push(SymbolEntry {
                description: Some(key_str.clone()),
                global_key: Some(key_str),
            });
            value::encode_symbol_handle(handle)
        },
    );
    linker.define(&mut store, "env", "symbol_for", symbol_for_fn)?;

    // ── Import 107: symbol_key_for(i64) → i64 ─────────────────────────
    let symbol_key_for_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, sym: i64| -> i64 {
            if !value::is_symbol(sym) {
                return make_type_error_exception(
                    &mut caller,
                    "TypeError: sym is not a Symbol",
                );
            }
            let handle = value::decode_symbol_handle(sym) as usize;
            let table = caller
                .data()
                .symbol_table.lock().unwrap_or_else(|e| e.into_inner());
            let key_to_return = table.get(handle).and_then(|entry| entry.global_key.clone());
            drop(table);
            if let Some(key) = key_to_return {
                return store_runtime_string(&mut caller, key);
            }
            value::encode_undefined()
        },
    );
    linker.define(&mut store, "env", "symbol_key_for", symbol_key_for_fn)?;

    // ECMAScript § 6.1.5.1 Well-Known Symbols
    // 返回预分配的 well-known symbol（id 与 symbol_table 启动条目索引一致）
    let symbol_well_known_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, id: i32| -> i64 {
            if id < 0 {
                return value::encode_undefined();
            }
            let table = caller
                .data()
                .symbol_table.lock().unwrap_or_else(|e| e.into_inner());
            let id_usize = id as usize;
            if id_usize < table.len() {
                value::encode_symbol_handle(id as u32)
            } else {
                value::encode_undefined()
            }
        },
    );
    linker.define(&mut store, "env", "symbol_well_known", symbol_well_known_fn)?;

    // ── Import 109: regex_create(i32, i32, i32, i32) → i64 ──────────────────────
    let regex_create_fn = Func::wrap(
        &mut store,
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

            // 标志校验
            const VALID_FLAGS: &[char] = &['d', 'g', 'i', 'm', 's', 'u', 'v', 'y'];
            let mut seen = [false; 128u8 as usize];
            for c in flags.chars() {
                if !VALID_FLAGS.contains(&c) {
                    *caller
                        .data()
                        .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!(
                        "SyntaxError: Invalid regular expression flag: '{}'",
                        c
                    ));
                    return value::encode_undefined();
                }
                let idx = c as usize;
                if idx < seen.len() {
                    if seen[idx] {
                        *caller
                            .data()
                            .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!(
                            "SyntaxError: Duplicate regular expression flag: '{}'",
                            c
                        ));
                        return value::encode_undefined();
                    }
                    seen[idx] = true;
                }
            }

            // u 和 v 标志不能同时出现（ECMAScript 22.2.2.1）
            if seen['u' as usize] && seen['v' as usize] {
                *caller
                    .data()
                    .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                    "SyntaxError: Invalid regular expression flags: u and v cannot be combined".to_string(),
                );
                return value::encode_undefined();
            }

            // 仅将引擎相关标志传给 regress
            let engine_flags: String = flags
                .chars()
                .filter(|c| matches!(c, 'i' | 'm' | 's' | 'u' | 'v'))
                .collect();

            // 编译正则表达式
            match regress::Regex::with_flags(&pattern, engine_flags.as_str()) {
                Ok(compiled) => {
                    let mut table = caller.data_mut().regex_table.lock().unwrap_or_else(|e| e.into_inner());
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
                    *caller
                        .data()
                        .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(format!("SyntaxError: Invalid regular expression: {}", e));
                    value::encode_undefined()
                }
            }
        },
    );
    linker.define(&mut store, "env", "regex_create", regex_create_fn)?;

    // ── Import 110: regex_test(i64, i64) → i64 ───────────────────────────────────
    let regex_test_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, regex_val: i64, str_val: i64| -> i64 {
            if !value::is_regexp(regex_val) {
                return value::encode_bool(false);
            }
            let handle = value::decode_regexp_handle(regex_val);

            // 获取字符串内容
            let s = get_string_value(&mut caller, str_val);

            // 单次锁定获取正则信息：is_global、is_sticky、start_pos、entry clone
            let (entry, is_global, is_sticky, start_pos) = {
                let table = caller.data().regex_table.lock().unwrap_or_else(|e| e.into_inner());
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
                let mut table = caller.data().regex_table.lock().unwrap_or_else(|e| e.into_inner());
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
    linker.define(&mut store, "env", "regex_test", regex_test_fn)?;
    /// 从 regress::Match 构建 RegExp 执行结果数组
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
            write_array_elem(caller, arr_ptr, i, elem);
        }
        write_array_length(caller, arr_ptr, group_count);
        // .index — 使用 m.start() 保持一致
        let index_val = value::encode_f64(m.start() as f64);
        let _ = define_host_data_property_from_caller(caller, arr_ptr as i64, "index", index_val);
        // .input
        let input_val = store_runtime_string(caller, s.to_string());
        let _ = define_host_data_property_from_caller(caller, arr_ptr as i64, "input", input_val);
        // .groups（collect 一次，供 .groups 和 .indices.groups 复用）
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
                write_array_elem(caller, indices_ptr, i, elem);
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

    // ── Import 111: regex_exec(i64, i64) → i64 ───────────────────────────────────
    let regex_exec_fn = Func::wrap(
        &mut store,
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
                let table = caller.data().regex_table.lock().unwrap_or_else(|e| e.into_inner());
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
                        let mut table = caller.data().regex_table.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(e) = table.get_mut(handle as usize) {
                            e.last_index = 0;
                        }
                    }
                    value::encode_null()
                }
                Some(m) => {
                    // 更新 lastIndex（全局或粘性模式）
                    if is_global || is_sticky {
                        let mut table = caller.data().regex_table.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(e) = table.get_mut(handle as usize) {
                            e.last_index = m.end() as i64;
                        }
                    }
                    build_match_result(
                        &mut caller,
                        &m,
                        &s,
                        (m.captures.len() + 1) as u32,
                        &entry.flags,
                    )
                }
                None => {
                    // 无匹配时重置 lastIndex
                    if is_global || is_sticky {
                        let mut table = caller.data().regex_table.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(e) = table.get_mut(handle as usize) {
                            e.last_index = 0;
                        }
                    }
                    value::encode_null()
                }
            }
        },
    );
    linker.define(&mut store, "env", "regex_exec", regex_exec_fn)?;

    // ── Import 112: string_match(i64, i64) → i64 ─────────────────────────────────
    let string_match_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, regexp: i64| -> i64 {
            // str.match(regexp)
            let s = get_string_value(&mut caller, receiver);

            if !value::is_regexp(regexp) {
                // 如果不是 RegExp，根据 ECMAScript 规范，将其转换为 RegExp
                // 相当于 new RegExp(regexp)
                let pattern = get_string_value(&mut caller, regexp);
                match regress::Regex::with_flags(&pattern, "") {
                    Ok(compiled) => {
                        // 临时 RegExp，仅在当次调用中使用，无需加入 regex_table（#236）
                        match compiled.find(&s) {
                            Some(m) => {
                                return build_match_result(
                                    &mut caller,
                                    &m,
                                    &s,
                                    (m.captures.len() + 1) as u32,
                                    "",
                                );
                            }
                            None => return value::encode_null(),
                        }
                    }
                    Err(e) => {
                        // 创建 RegExp 失败，抛出 SyntaxError
                        *caller
                            .data()
                            .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(format!("SyntaxError: Invalid regular expression: {}", e));
                        return value::encode_null();
                    }
                }
            }

            let handle = value::decode_regexp_handle(regexp);
            let (entry, is_global) = {
                let mut table = caller.data().regex_table.lock().unwrap_or_else(|e| e.into_inner());
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
                let is_sticky = entry.flags.contains('y');
                let last_idx = entry.last_index as usize;
                let match_result = if is_sticky {
                    entry.compiled.find_from(&s, last_idx).next()
                } else {
                    entry.compiled.find(&s)
                };
                match match_result {
                    Some(m) => {
                        if is_sticky && m.start() != last_idx {
                            value::encode_null()
                        } else {
                            build_match_result(
                                &mut caller,
                                &m,
                                &s,
                                (m.captures.len() + 1) as u32,
                                &entry.flags,
                            )
                        }
                    }
                    None => value::encode_null(),
                }
            }
        },
    );
    linker.define(&mut store, "env", "string_match", string_match_fn)?;

    // ── Import 114: string_search(i64, i64) → i64 ────────────────────────────────
    let string_search_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, regexp: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);

            // 根据 ECMAScript 22.1.3.21，非 RegExp 参数应转换为 RegExp
            if !value::is_regexp(regexp) {
                // 将参数转换为字符串，创建临时 RegExp 直接匹配，不存入 regex_table（#236）
                let pattern = get_string_value(&mut caller, regexp);
                return match regress::Regex::with_flags(&pattern, "") {
                    Ok(compiled) => match compiled.find(&s) {
                        Some(m) => value::encode_f64(m.start() as f64),
                        None => value::encode_f64(-1.0),
                    },
                    Err(e) => {
                        *caller
                            .data()
                            .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(format!("SyntaxError: Invalid regular expression: {}", e));
                        return value::encode_undefined();
                    }
                };
            }
            let handle = value::decode_regexp_handle(regexp);
            let (entry, prev_last_index) = {
                let mut table = caller.data().regex_table.lock().unwrap_or_else(|e| e.into_inner());
                let entry = match table.get_mut(handle as usize) {
                    Some(e) => e,
                    None => return value::encode_f64(-1.0),
                };
                let prev = entry.last_index;
                let is_global_or_sticky = entry.flags.contains('g') || entry.flags.contains('y');
                if is_global_or_sticky {
                    entry.last_index = 0;
                }
                (entry.clone(), prev)
            };

            let result = match entry.compiled.find(&s) {
                Some(m) => value::encode_f64(m.start() as f64),
                None => value::encode_f64(-1.0),
            };

            // 恢复 lastIndex（ECMAScript 22.2.6.11 §4）
            {
                let mut table = caller.data().regex_table.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(e) = table.get_mut(handle as usize) {
                    e.last_index = prev_last_index;
                }
            }
            result
        },
    );
    linker.define(&mut store, "env", "string_search", string_search_fn)?;

    // ── Import 115: string_split(i64, i64, i64) → i64 ────────────────────────────
    let string_split_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, sep: i64, limit: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);

            // 解析 limit（遵循 ECMAScript ToUint32 语义）
            let limit_val = if value::is_undefined(limit) {
                usize::MAX // undefined 表示无限制
            } else if value::is_f64(limit) {
                let n = value::decode_f64(limit);
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
                let table = caller.data().regex_table.lock().unwrap_or_else(|e| e.into_inner());
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
                    if start >= last_end {
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
                if parts.len() < limit_val && last_end <= s.len() {
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
    linker.define(&mut store, "env", "string_split", string_split_fn)?;

    Ok(())
}
