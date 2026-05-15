use wasmtime::*;
use wjsm_ir::value;
use num_bigint;
use num_traits::cast::ToPrimitive;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let abstract_eq = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            // 实现 Abstract Equality Comparison (ECMAScript 7.2.15)
            // 使用迭代而非递归来避免无限循环
            // 最多迭代 10 次防止死循环
            let mut x = a;
            let mut y = b;
            for _ in 0..10 {
                // 1. 同类型比较 → StrictEq
                if type_tag(x) == type_tag(y) {
                    return strict_eq(&mut caller, x, y);
                }

                // 2. null == undefined → true
                if value::is_null(x) && value::is_undefined(y) {
                    return value::encode_bool(true);
                }
                // 3. undefined == null → true
                if value::is_undefined(x) && value::is_null(y) {
                    return value::encode_bool(true);
                }

                // 4. Number == String → ToNumber(string) == number
                if value::is_f64(x) && value::is_string(y) {
                    y = to_number(&mut caller, y);
                    continue;
                }
                // 5. String == Number → string == ToNumber(number)
                if value::is_string(x) && value::is_f64(y) {
                    x = to_number(&mut caller, x);
                    continue;
                }

                // 6. Boolean == any → ToNumber(boolean) == any
                if value::is_bool(x) {
                    x = to_number(&mut caller, x);
                    continue;
                }
                // 7. any == Boolean → any == ToNumber(boolean)
                if value::is_bool(y) {
                    y = to_number(&mut caller, y);
                    continue;
                }

                // 8. Object == String/Number → ToPrimitive(object) == primitive
                if (value::is_object(x) || value::is_callable(x))
                    && (value::is_string(y) || value::is_f64(y))
                {
                    x = to_primitive(&mut caller, x);
                    continue;
                }
                // 9. String/Number == Object → primitive == ToPrimitive(object)
                if (value::is_string(x) || value::is_f64(x))
                    && (value::is_object(y) || value::is_callable(y))
                {
                    y = to_primitive(&mut caller, y);
                    continue;
                }

                // 10. BigInt == Number: 数学值比较 (ES §7.2.15)
                if value::is_bigint(x) && value::is_f64(y) {
                    let a_handle = value::decode_bigint_handle(x) as usize;
                    let b_f64 = f64::from_bits(y as u64);
                    // NaN 或 ±∞ → false
                    if !b_f64.is_finite() {
                        return value::encode_bool(false);
                    }
                    // 非整数 → false (BigInt 总是整数)
                    if b_f64.fract() != 0.0 {
                        return value::encode_bool(false);
                    }
                    // 通过 f64 → BigInt 转换比较数学值
                    if let Some(bi_y) = num_traits::cast::FromPrimitive::from_f64(b_f64) {
                        let table = caller
                            .data()
                            .bigint_table
                            .lock()
                            .expect("bigint_table mutex");
                        return value::encode_bool(
                            table.get(a_handle).map(|bi| *bi == bi_y).unwrap_or(false),
                        );
                    }
                    return value::encode_bool(false);
                }
                // 11. Number == BigInt
                if value::is_f64(x) && value::is_bigint(y) {
                    let a_f64 = f64::from_bits(x as u64);
                    let b_handle = value::decode_bigint_handle(y) as usize;
                    if !a_f64.is_finite() {
                        return value::encode_bool(false);
                    }
                    if a_f64.fract() != 0.0 {
                        return value::encode_bool(false);
                    }
                    if let Some(bi_x) = num_traits::cast::FromPrimitive::from_f64(a_f64) {
                        let table = caller
                            .data()
                            .bigint_table
                            .lock()
                            .expect("bigint_table mutex");
                        return value::encode_bool(
                            table.get(b_handle).map(|bi| *bi == bi_x).unwrap_or(false),
                        );
                    }
                    return value::encode_bool(false);
                }
                // 12. BigInt == String / String == BigInt: StringToBigInt → 比较 (ES §7.2.15)
                if value::is_bigint(x) && value::is_string(y) {
                    if let Some(bytes) = read_value_string_bytes(&mut caller, y) {
                        let s = String::from_utf8_lossy(&bytes)
                            .trim_end_matches('\0')
                            .to_string();
                        if let Ok(bi_y) = s.parse::<num_bigint::BigInt>() {
                            let a_handle = value::decode_bigint_handle(x) as usize;
                            let table = caller
                                .data()
                                .bigint_table
                                .lock()
                                .expect("bigint_table mutex");
                            return value::encode_bool(
                                table.get(a_handle).map(|bi| *bi == bi_y).unwrap_or(false),
                            );
                        }
                    }
                    return value::encode_bool(false);
                }
                if value::is_string(x) && value::is_bigint(y) {
                    if let Some(bytes) = read_value_string_bytes(&mut caller, x) {
                        let s = String::from_utf8_lossy(&bytes)
                            .trim_end_matches('\0')
                            .to_string();
                        if let Ok(bi_x) = s.parse::<num_bigint::BigInt>() {
                            let b_handle = value::decode_bigint_handle(y) as usize;
                            let table = caller
                                .data()
                                .bigint_table
                                .lock()
                                .expect("bigint_table mutex");
                            return value::encode_bool(
                                table.get(b_handle).map(|bi| *bi == bi_x).unwrap_or(false),
                            );
                        }
                    }
                    return value::encode_bool(false);
                }
                // 13. Symbol 与其他类型比较 → false
                if value::is_symbol(x) || value::is_symbol(y) {
                    return value::encode_bool(false);
                }
                // 14. 其他情况 → false
                return value::encode_bool(false);
            }
            // 迭代次数超限 → false
            value::encode_bool(false)
        },
    );

    // ── Import 20: abstract_compare(i64, i64) → i64 ──────────────────────────────

    let abstract_compare = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            // 实现 Abstract Relational Comparison (ECMAScript 7.2.17)
            // 返回值: true (a < b), false (a >= b 或无法比较)

            // 1. ToPrimitive(a, hint Number), ToPrimitive(b, hint Number)
            let pa = to_primitive(&mut caller, a);
            let pb = to_primitive(&mut caller, b);

            // 2. 若都是 String → 字典序比较
            if value::is_string(pa) && value::is_string(pb) {
                let a_str = get_string_value(&mut caller, pa);
                let b_str = get_string_value(&mut caller, pb);
                return value::encode_bool(a_str < b_str);
            }

            // 3. 否则 → ToNumber(px), ToNumber(py)
            let na = to_number(&mut caller, pa);
            let nb = to_number(&mut caller, pb);

            // 4. 若任一为 NaN → 返回 false
            let af = f64::from_bits(na as u64);
            let bf = f64::from_bits(nb as u64);
            if af.is_nan() || bf.is_nan() {
                return value::encode_bool(false);
            }

            // 5. 否则 → px < py 的数值比较
            value::encode_bool(af < bf)
        },
    );

    // ── Import 21: gc_collect(i32) → i32 ─────────────────────────────────────
    // 标记-清除 GC：尝试回收足够空间满足 requested_size。
    // 返回新的 heap_ptr 或 0（失败）。

    vec![
        (20, abstract_eq),
        (21, abstract_compare),
    ]
}
