use wasmtime::*;
use wjsm_ir::value;
use num_bigint;
use num_traits::cast::ToPrimitive;

use crate::types::*;
use crate::runtime::*;

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
            .expect("bigint_table mutex");
        (table.get(a_handle).cloned(), table.get(b_handle).cloned())
    };
    match (a_val, b_val) {
        (Some(av), Some(bv)) => {
            let result = op(&av, &bv);
            let mut table = caller
                .data()
                .bigint_table
                .lock()
                .expect("bigint_table mutex");
            let handle = table.len() as u32;
            table.push(result);
            value::encode_bigint_handle(handle)
        }
        _ => value::encode_undefined(),
    }
}

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let bigint_from_literal_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, ptr: i32, _len: i32| -> i64 {
            let s = read_string(&mut caller, ptr as u32).unwrap_or_default();
            // 去掉末尾可能存在的 nul 字符
            let trimmed = s.trim_end_matches('\0');
            if let Ok(bigint) = trimmed.parse::<num_bigint::BigInt>() {
                let mut table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .expect("bigint_table mutex");
                let handle = table.len() as u32;
                table.push(bigint);
                value::encode_bigint_handle(handle)
            } else {
                value::encode_undefined()
            }
        },
    );

    // ── BigInt arithmetic helpers ─────────────────────────────────────

    let bigint_add_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            bigint_binary_op(&mut caller, a, b, |x, y| x + y)
        },
    );

    let bigint_sub_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            bigint_binary_op(&mut caller, a, b, |x, y| x - y)
        },
    );

    let bigint_mul_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            bigint_binary_op(&mut caller, a, b, |x, y| x * y)
        },
    );

    let bigint_div_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let (av, bv) = {
                let table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .expect("bigint_table mutex");
                (table.get(a_handle).cloned(), table.get(b_handle).cloned())
            };
            match (av, bv) {
                (Some(x), Some(y)) => {
                    if y == 0u32.into() {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .expect("runtime error mutex") =
                            Some("RangeError: BigInt division by zero".to_string());
                        return value::encode_undefined();
                    }
                    let result = x / y;
                    let mut table = caller
                        .data()
                        .bigint_table
                        .lock()
                        .expect("bigint_table mutex");
                    let handle = table.len() as u32;
                    table.push(result);
                    value::encode_bigint_handle(handle)
                }
                _ => value::encode_undefined(),
            }
        },
    );

    let bigint_mod_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let (av, bv) = {
                let table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .expect("bigint_table mutex");
                (table.get(a_handle).cloned(), table.get(b_handle).cloned())
            };
            match (av, bv) {
                (Some(x), Some(y)) => {
                    if y == 0u32.into() {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .expect("runtime error mutex") =
                            Some("RangeError: BigInt division by zero".to_string());
                        return value::encode_undefined();
                    }
                    let result = x % y;
                    let mut table = caller
                        .data()
                        .bigint_table
                        .lock()
                        .expect("bigint_table mutex");
                    let handle = table.len() as u32;
                    table.push(result);
                    value::encode_bigint_handle(handle)
                }
                _ => value::encode_undefined(),
            }
        },
    );

    let bigint_pow_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let (av, bv) = {
                let table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .expect("bigint_table mutex");
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
                            .expect("runtime error mutex") =
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
                                .expect("runtime error mutex") =
                                Some("RangeError: BigInt exponent too large".to_string());
                            return value::encode_undefined();
                        }
                    };
                    let result = x.pow(exp);
                    let mut table = caller
                        .data()
                        .bigint_table
                        .lock()
                        .expect("bigint_table mutex");
                    let handle = table.len() as u32;
                    table.push(result);
                    value::encode_bigint_handle(handle)
                }
                _ => value::encode_undefined(),
            }
        },
    );

    let bigint_neg_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, a: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let a_val = {
                let table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .expect("bigint_table mutex");
                table.get(a_handle).cloned()
            };
            if let Some(av) = a_val {
                let result = -av;
                let mut table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .expect("bigint_table mutex");
                let handle = table.len() as u32;
                table.push(result);
                value::encode_bigint_handle(handle)
            } else {
                value::encode_undefined()
            }
        },
    );

    let bigint_eq_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let eq = {
                let table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .expect("bigint_table mutex");
                table
                    .get(a_handle)
                    .zip(table.get(b_handle))
                    .map(|(x, y)| x == y)
                    .unwrap_or(false)
            };
            value::encode_bool(eq)
        },
    );

    let bigint_cmp_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let cmp = {
                let table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .expect("bigint_table mutex");
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

    // ═══════════════════════════════════════════════════════════════════
    // ── Symbol host functions ──────────────────────────────────────────
    // ═══════════════════════════════════════════════════════════════════

    // ── Import 105: symbol_create(i64) → i64 ──────────────────────────

    vec![
        (95, bigint_from_literal_fn),
        (96, bigint_add_fn),
        (97, bigint_sub_fn),
        (98, bigint_mul_fn),
        (99, bigint_div_fn),
        (100, bigint_mod_fn),
        (101, bigint_pow_fn),
        (102, bigint_neg_fn),
        (103, bigint_eq_fn),
        (104, bigint_cmp_fn),
    ]
}
