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
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
                .bigint_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (table.get(a_handle).cloned(), table.get(b_handle).cloned())
        };
        match (a_val, b_val) {
            (Some(av), Some(bv)) => {
                let result = op(&av, &bv);
                let mut table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                (table.get(a_handle).cloned(), table.get(b_handle).cloned())
            };
            match (av, bv) {
                (Some(x), Some(y)) => {
                    if y == 0u32.into() {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) =
                            Some("RangeError: BigInt division by zero".to_string());
                        return value::encode_undefined();
                    }
                    let result = x / y;
                    let mut table = caller
                        .data()
                        .bigint_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
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
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                (table.get(a_handle).cloned(), table.get(b_handle).cloned())
            };
            match (av, bv) {
                (Some(x), Some(y)) => {
                    if y == 0u32.into() {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) =
                            Some("RangeError: BigInt division by zero".to_string());
                        return value::encode_undefined();
                    }
                    let result = x % y;
                    let mut table = caller
                        .data()
                        .bigint_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
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
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                (table.get(a_handle).cloned(), table.get(b_handle).cloned())
            };
            match (av, bv) {
                (Some(x), Some(y)) => {
                    // Per spec §6.1.6.1.9: negative exponent throws RangeError
                    if y.sign() == num_bigint::Sign::Minus {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) =
                            Some("RangeError: BigInt exponent must be non-negative".to_string());
                        return value::encode_undefined();
                    }
                    let exp = match y.to_u32() {
                        Some(e) => e,
                        None => {
                            *caller
                                .data()
                                .runtime_error
                                .lock()
                                .unwrap_or_else(|e| e.into_inner()) =
                                Some("RangeError: BigInt exponent too large".to_string());
                            return value::encode_undefined();
                        }
                    };
                    let result = x.pow(exp);
                    let mut table = caller
                        .data()
                        .bigint_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
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
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(a_handle).cloned()
            };
            if let Some(av) = a_val {
                let result = -av;
                let mut table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let handle = table.len() as u32;
                table.push(result);
                value::encode_bigint_handle(handle)
            } else {
                value::encode_undefined()
            }
        },
    );
    fn bigint_push_result(
        caller: &mut Caller<'_, RuntimeState>,
        result: num_bigint::BigInt,
    ) -> i64 {
        let mut table = caller
            .data()
            .bigint_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(result);
        value::encode_bigint_handle(handle)
    }

    /// ES BigInt 移位：ℝ(y) mod 2^64，且结果非负。
    fn bigint_shift_amount(y: &num_bigint::BigInt) -> Result<u64, &'static str> {
        let modulus = num_bigint::BigInt::from(1u64) << 64;
        let reduced: num_bigint::BigInt = y % &modulus;
        if reduced.sign() == num_bigint::Sign::Minus {
            return Err("RangeError: BigInt shift amount must be non-negative");
        }
        reduced
            .to_u64()
            .ok_or("RangeError: BigInt shift amount too large")
    }

    linker.define(&mut store, "env", "bigint_neg", bigint_neg_fn)?;
    let bigint_bit_and_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            bigint_binary_op(&mut caller, a, b, |x, y| x & y)
        },
    );
    linker.define(&mut store, "env", "bigint_bit_and", bigint_bit_and_fn)?;
    let bigint_bit_or_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            bigint_binary_op(&mut caller, a, b, |x, y| x | y)
        },
    );
    linker.define(&mut store, "env", "bigint_bit_or", bigint_bit_or_fn)?;
    let bigint_bit_xor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            bigint_binary_op(&mut caller, a, b, |x, y| x ^ y)
        },
    );
    linker.define(&mut store, "env", "bigint_bit_xor", bigint_bit_xor_fn)?;
    let bigint_shl_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let (a_val, b_val) = {
                let table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                (table.get(a_handle).cloned(), table.get(b_handle).cloned())
            };
            match (a_val, b_val) {
                (Some(x), Some(y)) => match bigint_shift_amount(&y) {
                    Ok(shift) => bigint_push_result(&mut caller, x << shift),
                    Err(msg) => {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(msg.to_string());
                        value::encode_undefined()
                    }
                },
                _ => value::encode_undefined(),
            }
        },
    );
    linker.define(&mut store, "env", "bigint_shl", bigint_shl_fn)?;
    let bigint_shr_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let (a_val, b_val) = {
                let table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                (table.get(a_handle).cloned(), table.get(b_handle).cloned())
            };
            match (a_val, b_val) {
                (Some(x), Some(y)) => match bigint_shift_amount(&y) {
                    Ok(shift) => bigint_push_result(&mut caller, x >> shift),
                    Err(msg) => {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(msg.to_string());
                        value::encode_undefined()
                    }
                },
                _ => value::encode_undefined(),
            }
        },
    );
    linker.define(&mut store, "env", "bigint_shr", bigint_shr_fn)?;
    let bigint_bit_not_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let a_val = {
                let table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(a_handle).cloned()
            };
            if let Some(av) = a_val {
                bigint_push_result(&mut caller, !av)
            } else {
                value::encode_undefined()
            }
        },
    );
    linker.define(&mut store, "env", "bigint_bit_not", bigint_bit_not_fn)?;
    let bigint_eq_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let eq = {
                let table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
                Some(get_string_utf8_lossy(&mut caller, desc))
            } else {
                Some(render_value(&mut caller, desc).unwrap_or_default())
            };
            let mut table = caller
                .data()
                .symbol_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
                .symbol_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
                return make_type_error_exception(&mut caller, "TypeError: sym is not a Symbol");
            }
            let handle = value::decode_symbol_handle(sym) as usize;
            let table = caller
                .data()
                .symbol_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
                .symbol_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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

            regexp_create_from_parts(&mut caller, pattern, flags)
        },
    );
    linker.define(&mut store, "env", "regex_create", regex_create_fn)?;

    // ── Import 110: regex_test(i64, i64) → i64 ───────────────────────────────────
    let regex_test_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, regex_val: i64, str_val: i64| -> i64 {
            regexp_test_impl(&mut caller, regex_val, str_val)
        },
    );
    linker.define(&mut store, "env", "regex_test", regex_test_fn)?;

    // ── Import 111: regex_exec(i64, i64) → i64 ───────────────────────────────────
    let regex_exec_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, regex_val: i64, str_val: i64| -> i64 {
            regexp_exec_impl(&mut caller, regex_val, str_val)
        },
    );
    linker.define(&mut store, "env", "regex_exec", regex_exec_fn)?;
    Ok(())
}
