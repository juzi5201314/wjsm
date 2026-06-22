use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::*;
/// Maximum array length per ECMAScript (2^32 - 1).
const MAX_ARRAY_LENGTH: u32 = u32::MAX;
const ARRAY_LENGTH_RANGE_ERROR: &str = "Invalid array length";

fn array_length_would_overflow(len: u32, add: u32) -> bool {
    len.checked_add(add).is_none_or(|n| n > MAX_ARRAY_LENGTH)
}

pub(crate) fn push_array_value(caller: &mut Caller<'_, RuntimeState>, arr: i64, val: i64) -> Result<(), i64> {
    let mut ptr = resolve_array_ptr(caller, arr).ok_or_else(value::encode_undefined)?;
    let len = read_array_length(caller, ptr).unwrap_or(0);
    if array_length_would_overflow(len, 1) {
        return Err(make_range_error_exception(caller, ARRAY_LENGTH_RANGE_ERROR));
    }
    let cap = read_array_capacity(caller, ptr).unwrap_or(0);
    if len >= cap {
        let new_cap = cap.max(1) * 2;
        ptr = grow_array(caller, ptr, arr, new_cap.max(len + 1))
            .ok_or_else(value::encode_undefined)?;
    }
    write_array_elem(caller, ptr, len, val);
    write_array_length(caller, ptr, len + 1);
    Ok(())
}

async fn push_iterator_values_async(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    iterator: i64,
) -> bool {
    let Some(iter_ptr) = resolve_handle(caller, iterator) else {
        return false;
    };
    let Some(next) = read_object_property_by_name(caller, iter_ptr, "next") else {
        return false;
    };
    if !value::is_callable(next) {
        return false;
    }
    loop {
        let result =
            call_iterator_method_async(caller, next, iterator, value::encode_undefined()).await;

        // A4: 若 next() 同步抛出（返回 TAG_EXCEPTION），用真实错误消息替换误导的 "not iterable"。
        // 注：表达式位 spread 无 IsException 分叉，无法做到可捕获；仅改进延迟错误消息的准确性。
        if value::is_exception(result) {
            let reason = exception_reason(caller, result);
            let msg = render_value(caller, reason).unwrap_or_else(|_| "unknown error".to_string());
            set_runtime_error(
                caller.data(),
                format!("TypeError: iterator.next() threw: {}", msg),
            );
            return false;
        }

        let Some(result_ptr) = resolve_handle(caller, result) else {
            return false;
        };
        let done = read_object_property_by_name(caller, result_ptr, "done")
            .map(nanbox_to_bool)
            .unwrap_or(true);
        if done {
            return true;
        }
        let val = read_object_property_by_name(caller, result_ptr, "value")
            .unwrap_or_else(value::encode_undefined);
        if push_array_value(caller, arr, val).is_err() {
            set_runtime_error(caller.data(), ARRAY_LENGTH_RANGE_ERROR.to_string());
            return false;
        }
    }
}

pub(crate) async fn array_push_spread_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    iterable: i64,
) -> i64 {
    if value::is_array(iterable)
        && let Some(ptr) = resolve_array_ptr(caller, iterable)
    {
        let len = read_array_length(caller, ptr).unwrap_or(0);
        for i in 0..len {
            let val = read_array_elem(caller, ptr, i).unwrap_or_else(value::encode_undefined);
            if push_array_value(caller, arr, val).is_err() {
                set_runtime_error(caller.data(), ARRAY_LENGTH_RANGE_ERROR.to_string());
                return value::encode_undefined();
            }
        }
        return value::encode_undefined();
    }

    if let Some(bytes) = read_value_string_bytes(caller, iterable) {
        for byte in bytes {
            let val = store_runtime_string(caller, (byte as char).to_string());
            if push_array_value(caller, arr, val).is_err() {
                set_runtime_error(caller.data(), ARRAY_LENGTH_RANGE_ERROR.to_string());
                return value::encode_undefined();
            }
        }
        return value::encode_undefined();
    }

    if let Some(ptr) = resolve_handle(caller, iterable)
        && let Some(method) = read_iterator_method(caller, ptr)
    {
        let iterator = call_iterable_method_async(caller, method, iterable).await;
        if push_iterator_values_async(caller, arr, iterator).await {
            return value::encode_undefined();
        }
    }

    set_runtime_error(
        caller.data(),
        "TypeError: value is not iterable".to_string(),
    );
    value::encode_undefined()
}

pub(crate) fn define_array_object(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let arr_proto_push_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let count = args_count as u32;
            if array_length_would_overflow(len, count) {
                return make_range_error_exception(&mut caller, ARRAY_LENGTH_RANGE_ERROR);
            }
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0);
            let mut ptr = ptr;
            if len + count > cap {
                let new_cap = cap.max(1) * 2;
                let needed = (len + count).max(new_cap);
                if let Some(new_ptr) = grow_array(&mut caller, ptr, this_val, needed) {
                    ptr = new_ptr;
                } else {
                    return value::encode_undefined();
                }
            }
            for i in 0..count {
                let val = read_shadow_arg(&mut caller, args_base, i);
                write_array_elem(&mut caller, ptr, len + i, val);
            }
            write_array_length(&mut caller, ptr, len + count);
            value::encode_f64((len + count) as f64)
        },
    );
    linker.define(&mut store, "env", "arr_proto_push", arr_proto_push_fn)?;

    // ── arr_proto_pop (#50) ───────────────────────────────────────────
    let arr_proto_pop_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         _args_base: i32,
         _args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            if len == 0 {
                return value::encode_undefined();
            }
            let new_len = len - 1;
            let val =
                read_array_elem(&mut caller, ptr, new_len).unwrap_or(value::encode_undefined());
            write_array_length(&mut caller, ptr, new_len);
            val
        },
    );
    linker.define(&mut store, "env", "arr_proto_pop", arr_proto_pop_fn)?;

    // ── arr_proto_includes (#51) ──────────────────────────────────────
    let arr_proto_includes_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         _args_count: i32|
         -> i64 {
            let val = read_shadow_arg(&mut caller, args_base, 0);
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_bool(false);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i)
                    && elem == val
                {
                    return value::encode_bool(true);
                }
            }
            value::encode_bool(false)
        },
    );
    linker.define(
        &mut store,
        "env",
        "arr_proto_includes",
        arr_proto_includes_fn,
    )?;

    // ── arr_proto_index_of (#52) ──────────────────────────────────────
    let arr_proto_index_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         _args_count: i32|
         -> i64 {
            let val = read_shadow_arg(&mut caller, args_base, 0);
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_f64(-1.0);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i)
                    && elem == val
                {
                    return value::encode_f64(i as f64);
                }
            }
            value::encode_f64(-1.0)
        },
    );
    linker.define(
        &mut store,
        "env",
        "arr_proto_index_of",
        arr_proto_index_of_fn,
    )?;

    // ── arr_proto_join (#53) ─────────────────────────────────────────
    let arr_proto_join_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let sep_val = if args_count > 0 {
                read_shadow_arg(&mut caller, args_base, 0)
            } else {
                value::encode_undefined()
            };
            // 默认分隔符为 ","
            let sep_str = if value::is_undefined(sep_val) || value::is_null(sep_val) {
                ",".to_string()
            } else {
                get_string_value(&mut caller, sep_val)
            };
            let mut parts = Vec::new();
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i) {
                    parts.push(render_value(&mut caller, elem).unwrap_or_else(|_| "".to_string()));
                } else {
                    parts.push(String::new());
                }
            }
            store_runtime_string(&caller, parts.join(&sep_str))
        },
    );
    linker.define(&mut store, "env", "arr_proto_join", arr_proto_join_fn)?;

    // ── arr_proto_concat (#54) ────────────────────────────────────────
    let arr_proto_concat_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(this_ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let this_len = read_array_length(&mut caller, this_ptr).unwrap_or(0);
            // 计算总元素数
            let mut total_len = this_len as usize;
            for i in 0..args_count as u32 {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                if value::is_array(arg) {
                    if let Some(arg_ptr) = resolve_array_ptr(&mut caller, arg) {
                        total_len += read_array_length(&mut caller, arg_ptr).unwrap_or(0) as usize;
                    }
                } else {
                    total_len += 1;
                }
            }
            let new_arr = array_species_create(&mut caller, this_val, total_len as u32);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            let mut write_idx = 0u32;
            // 复制 this 元素
            for i in 0..this_len {
                if let Some(elem) = read_array_elem(&mut caller, this_ptr, i) {
                    write_array_elem(&mut caller, new_ptr, write_idx, elem);
                    write_idx += 1;
                }
            }
            // 复制参数元素
            for i in 0..args_count as u32 {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                if value::is_array(arg) {
                    if let Some(arg_ptr) = resolve_array_ptr(&mut caller, arg) {
                        let arg_len = read_array_length(&mut caller, arg_ptr).unwrap_or(0);
                        for j in 0..arg_len {
                            if let Some(elem) = read_array_elem(&mut caller, arg_ptr, j) {
                                write_array_elem(&mut caller, new_ptr, write_idx, elem);
                                write_idx += 1;
                            }
                        }
                    }
                } else {
                    write_array_elem(&mut caller, new_ptr, write_idx, arg);
                    write_idx += 1;
                }
            }
            write_array_length(&mut caller, new_ptr, write_idx);
            new_arr
        },
    );
    linker.define(&mut store, "env", "arr_proto_concat", arr_proto_concat_fn)?;

    // ── arr_proto_slice (#55) ─────────────────────────────────────────
    let arr_proto_slice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            let start = if args_count > 0 {
                let s_f64 = value::decode_f64(read_shadow_arg(&mut caller, args_base, 0));
                if s_f64.is_nan() {
                    0
                } else if s_f64 < 0.0 {
                    (len + s_f64 as i32).max(0)
                } else {
                    (s_f64 as i32).min(len)
                }
            } else {
                0
            };
            let end = if args_count > 1 {
                let e_f64 = value::decode_f64(read_shadow_arg(&mut caller, args_base, 1));
                if e_f64.is_nan() {
                    len
                } else if e_f64 < 0.0 {
                    (len + e_f64 as i32).max(0)
                } else {
                    (e_f64 as i32).min(len)
                }
            } else {
                len
            };
            let count = (end - start).max(0) as u32;
            let new_arr = array_species_create(&mut caller, this_val, count);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for i in 0..count {
                let elem = read_array_elem(&mut caller, ptr, start as u32 + i)
                    .unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, new_ptr, i, elem);
            }
            write_array_length(&mut caller, new_ptr, count);
            new_arr
        },
    );
    linker.define(&mut store, "env", "arr_proto_slice", arr_proto_slice_fn)?;

    // ── arr_proto_fill (#56) ──────────────────────────────────────────
    let arr_proto_fill_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let val = read_shadow_arg(&mut caller, args_base, 0);
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return this_val;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            let start = if args_count > 1 {
                let s_f64 = value::decode_f64(read_shadow_arg(&mut caller, args_base, 1));
                if s_f64.is_nan() {
                    0
                } else if s_f64 < 0.0 {
                    (len + s_f64 as i32).max(0)
                } else {
                    (s_f64 as i32).min(len)
                }
            } else {
                0
            };
            let end = if args_count > 2 {
                let e_f64 = value::decode_f64(read_shadow_arg(&mut caller, args_base, 2));
                if e_f64.is_nan() {
                    len
                } else if e_f64 < 0.0 {
                    (len + e_f64 as i32).max(0)
                } else {
                    (e_f64 as i32).min(len)
                }
            } else {
                len
            };
            for i in start..end {
                write_array_elem(&mut caller, ptr, i as u32, val);
            }
            this_val
        },
    );
    linker.define(&mut store, "env", "arr_proto_fill", arr_proto_fill_fn)?;

    // ── arr_proto_reverse (#57) ───────────────────────────────────────
    let arr_proto_reverse_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         _args_base: i32,
         _args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return this_val;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len / 2 {
                let a = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let b = read_array_elem(&mut caller, ptr, len - 1 - i)
                    .unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, ptr, i, b);
                write_array_elem(&mut caller, ptr, len - 1 - i, a);
            }
            this_val
        },
    );
    linker.define(&mut store, "env", "arr_proto_reverse", arr_proto_reverse_fn)?;

    // ── arr_proto_flat (#58) ──────────────────────────────────────────
    let arr_proto_flat_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            // depth: default 1; ToIntegerOrInfinity; depth <= 0 means no flattening
            let depth = if args_count > 0 {
                let d = value::decode_f64(read_shadow_arg(&mut caller, args_base, 0));
                if d.is_nan() {
                    0
                } else {
                    let i = d.trunc() as i64;
                    if i <= 0 {
                        0
                    } else {
                        i as u32
                    }
                }
            } else {
                1
            };
            // 递归展平
            fn flatten(
                caller: &mut Caller<'_, RuntimeState>,
                arr: i64,
                depth: u32,
                elements: &mut Vec<i64>,
            ) {
                let Some(ptr) = resolve_array_ptr(caller, arr) else {
                    elements.push(arr);
                    return;
                };
                let len = read_array_length(caller, ptr).unwrap_or(0);
                for i in 0..len {
                    if let Some(elem) = read_array_elem(caller, ptr, i) {
                        if depth > 0 && value::is_array(elem) {
                            flatten(caller, elem, depth - 1, elements);
                        } else {
                            elements.push(elem);
                        }
                    }
                }
            }
            let mut elements = Vec::new();
            flatten(&mut caller, this_val, depth, &mut elements);
            let new_arr = array_species_create(&mut caller, this_val, elements.len() as u32);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for (i, elem) in elements.iter().enumerate() {
                write_array_elem(&mut caller, new_ptr, i as u32, *elem);
            }
            write_array_length(&mut caller, new_ptr, elements.len() as u32);
            new_arr
        },
    );
    linker.define(&mut store, "env", "arr_proto_flat", arr_proto_flat_fn)?;

    // ── arr_proto_shift (#59) ─────────────────────────────────────────
    let arr_proto_shift_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         _args_base: i32,
         _args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            if len == 0 {
                return value::encode_undefined();
            }
            let val = read_array_elem(&mut caller, ptr, 0).unwrap_or(value::encode_undefined());
            // 左移元素
            for i in 1..len {
                let elem =
                    read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, ptr, i - 1, elem);
            }
            write_array_length(&mut caller, ptr, len - 1);
            val
        },
    );
    linker.define(&mut store, "env", "arr_proto_shift", arr_proto_shift_fn)?;

    // ── arr_proto_unshift (#60) ───────────────────────────────────────
    let arr_proto_unshift_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let add = args_count as u32;
            if array_length_would_overflow(len, add) {
                return make_range_error_exception(&mut caller, ARRAY_LENGTH_RANGE_ERROR);
            }
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0);
            let new_len = len + add;
            let mut ptr = ptr;
            if new_len > cap {
                let new_cap = cap.max(1) * 2;
                let needed = new_len.max(new_cap);
                if let Some(new_ptr) = grow_array(&mut caller, ptr, this_val, needed) {
                    ptr = new_ptr;
                } else {
                    return value::encode_undefined();
                }
            }
            // 右移现有元素
            for i in (0..len).rev() {
                let elem =
                    read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, ptr, i + args_count as u32, elem);
            }
            // 在前面插入新元素
            for i in 0..args_count as u32 {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                write_array_elem(&mut caller, ptr, i, arg);
            }
            write_array_length(&mut caller, ptr, new_len);
            value::encode_f64(new_len as f64)
        },
    );
    linker.define(&mut store, "env", "arr_proto_unshift", arr_proto_unshift_fn)?;

    // ── arr_proto_at (#62) ────────────────────────────────────────────
    let arr_proto_at_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            let idx = if args_count > 0 {
                let i_f64 = value::decode_f64(read_shadow_arg(&mut caller, args_base, 0));
                // ToIntegerOrInfinity(NaN) => 0 (ES2024 §6.1.6)
                if i_f64.is_nan() {
                    0
                } else if i_f64 < 0.0 {
                    len + i_f64 as i32
                } else {
                    i_f64 as i32
                }
            } else {
                0
            };
            if idx < 0 || idx >= len {
                return value::encode_undefined();
            }
            read_array_elem(&mut caller, ptr, idx as u32).unwrap_or(value::encode_undefined())
        },
    );
    linker.define(&mut store, "env", "arr_proto_at", arr_proto_at_fn)?;

    // ── arr_proto_copy_within (#63) ──────────────────────────────────
    let arr_proto_copy_within_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return this_val;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            // target
            let raw_target = if args_count > 0 {
                let t = value::decode_f64(read_shadow_arg(&mut caller, args_base, 0));
                if t.is_nan() { 0 } else { t as i32 }
            } else {
                0
            };
            let target = if raw_target < 0 {
                (len + raw_target).max(0)
            } else {
                raw_target.min(len)
            };
            // start
            let raw_start = if args_count > 1 {
                let s = value::decode_f64(read_shadow_arg(&mut caller, args_base, 1));
                if s.is_nan() { 0 } else { s as i32 }
            } else {
                0
            };
            let start = if raw_start < 0 {
                (len + raw_start).max(0)
            } else {
                raw_start.min(len)
            };
            // end
            let raw_end = if args_count > 2 {
                let e = value::decode_f64(read_shadow_arg(&mut caller, args_base, 2));
                if e.is_nan() { len } else { e as i32 }
            } else {
                len
            };
            let end = if raw_end < 0 {
                (len + raw_end).max(0)
            } else {
                raw_end.min(len)
            };
            let count = (end - start).min(len - target).max(0) as u32;
            // 复制元素（处理重叠：从后往前复制；源为 hole 时目标也为 hole）
            if target < start {
                for i in 0..count {
                    let from = (start as u32) + i;
                    let to = (target as u32) + i;
                    if array_elem_present(&mut caller, ptr, from) {
                        let elem = read_array_elem(&mut caller, ptr, from)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(&mut caller, ptr, to, elem);
                    } else {
                        write_array_hole(&mut caller, ptr, to);
                    }
                }
            } else {
                for i in (0..count).rev() {
                    let from = (start as u32) + i;
                    let to = (target as u32) + i;
                    if array_elem_present(&mut caller, ptr, from) {
                        let elem = read_array_elem(&mut caller, ptr, from)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(&mut caller, ptr, to, elem);
                    } else {
                        write_array_hole(&mut caller, ptr, to);
                    }
                }
            }
            this_val
        },
    );
    linker.define(
        &mut store,
        "env",
        "arr_proto_copy_within",
        arr_proto_copy_within_fn,
    )?;

    // ── arr_proto_splice (#74) ───────────────────────────────────────
    let arr_proto_splice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            // 读取 start
            let raw_start = if args_count > 0 {
                let s = value::decode_f64(read_shadow_arg(&mut caller, args_base, 0));
                if s.is_nan() { 0 } else { s as i32 }
            } else {
                0
            };
            let start_idx = if raw_start < 0 {
                (len + raw_start).max(0)
            } else {
                raw_start.min(len)
            };
            // 读取 deleteCount
            let delete_count = if args_count > 1 {
                let d = value::decode_f64(read_shadow_arg(&mut caller, args_base, 1));
                if d.is_nan() { 0 } else { (d as i32).max(0) }
            } else {
                (len - start_idx).max(0)
            };
            let actual_delete = delete_count.min(len - start_idx);
            let insert_count = (args_count - 2).max(0);
            let new_len = len - actual_delete + insert_count;
            if new_len < 0 || new_len as u64 > u64::from(MAX_ARRAY_LENGTH) {
                return make_range_error_exception(&mut caller, ARRAY_LENGTH_RANGE_ERROR);
            }
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0) as i32;
            let mut ptr = ptr;
            if new_len > cap {
                let new_cap = cap.max(1) * 2;
                let needed = new_len.max(new_cap);
                if let Some(new_ptr) = grow_array(&mut caller, ptr, this_val, needed as u32) {
                    ptr = new_ptr;
                } else {
                    return value::encode_undefined();
                }
            }
            // 收集被删除的元素
            let deleted_arr = array_species_create(&mut caller, this_val, actual_delete as u32);
            let Some(deleted_ptr) = resolve_array_ptr(&mut caller, deleted_arr) else {
                return value::encode_undefined();
            };
            for i in 0..actual_delete {
                let elem = read_array_elem(&mut caller, ptr, (start_idx as u32) + i as u32)
                    .unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, deleted_ptr, i as u32, elem);
            }
            write_array_length(&mut caller, deleted_ptr, actual_delete as u32);
            // 移动元素（右移或左移）
            if insert_count != actual_delete {
                if insert_count < actual_delete {
                    // 左移
                    for i in start_idx..(len - actual_delete + insert_count) {
                        let src = i + actual_delete - insert_count;
                        let elem = read_array_elem(&mut caller, ptr, src as u32)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(&mut caller, ptr, i as u32, elem);
                    }
                } else {
                    // 右移（从后往前）
                    for i in (start_idx..(len - actual_delete + insert_count)).rev() {
                        let src = i - insert_count + actual_delete;
                        let elem = read_array_elem(&mut caller, ptr, src as u32)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(
                            &mut caller,
                            ptr,
                            i as u32 + insert_count as u32 - actual_delete as u32,
                            elem,
                        );
                    }
                }
            }
            // 插入新元素
            for i in 0..insert_count {
                let item = read_shadow_arg(&mut caller, args_base, 2 + i as u32);
                write_array_elem(&mut caller, ptr, (start_idx as u32) + i as u32, item);
            }
            write_array_length(&mut caller, ptr, new_len as u32);
            deleted_arr
        },
    );
    linker.define(&mut store, "env", "arr_proto_splice", arr_proto_splice_fn)?;

    // ── arr_proto_is_array (#75) ──────────────────────────────────────
    let arr_proto_is_array_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         _this_val: i64,
         args_base: i32,
         _args_count: i32|
         -> i64 {
            let val = read_shadow_arg(&mut caller, args_base, 0);
            value::encode_bool(value::is_array(val))
        },
    );
    linker.define(
        &mut store,
        "env",
        "arr_proto_is_array",
        arr_proto_is_array_fn,
    )?;

    // ── abort_shadow_stack_overflow (#76) ─────────────────────────────
    let abort_shadow_stack_overflow_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, shadow_sp: i32, args_bytes: i32, stack_end: i32| {
            let mut buffer = caller
                .data()
                .output
                .lock()
                .expect("runtime output buffer mutex should not be poisoned");
            writeln!(
                &mut *buffer,
                "shadow stack overflow: sp=0x{shadow_sp:x} + {args_bytes} bytes > end=0x{stack_end:x}"
            ).ok();
            *caller
                .data()
                .runtime_error
                .lock()
                .expect("runtime error mutex") = Some(format!(
                "shadow stack overflow: sp={shadow_sp} + {args_bytes} > end={stack_end}"
            ));
        },
    );
    linker.define(
        &mut store,
        "env",
        "abort_shadow_stack_overflow",
        abort_shadow_stack_overflow_fn,
    )?;

    // ── func_bind (#80): Function.prototype.bind ────────────────────────────
    let func_bind_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         func: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 { func_bind_impl(&mut caller, func, this_val, args_base, args_count) },
    );
    linker.define(&mut store, "env", "func_bind", func_bind_fn)?;

    // ── object_rest (#81): Exclude specified keys from object ───────────────
    let object_rest_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, excluded_keys: i64| -> i64 {
            object_rest_impl(&mut caller, obj, excluded_keys)
        },
    );
    linker.define(&mut store, "env", "object_rest", object_rest_fn)?;

    // ── obj_spread (#82): Copy own enumerable properties ────────────────────
    let obj_spread_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, dest: i64, source: i64| {
            obj_spread_impl(&mut caller, dest, source);
        },
    );
    linker.define(&mut store, "env", "obj_spread", obj_spread_fn)?;

    // ── Import 83: has_own_property(i64, i32) -> i64 ──────────────────────────
    let has_own_property_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_ptr: i32| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) && !value::is_array(obj) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: hasOwnProperty called on non-object".to_string());
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_bool(false);
            };
            let found = find_property_slot_by_name_id(&mut caller, ptr, key_ptr as u32);
            value::encode_bool(found.is_some())
        },
    );
    linker.define(&mut store, "env", "has_own_property", has_own_property_fn)?;
    // ── Import 85: obj_values(i64) -> i64 ─────────────────────────────────────
    let obj_values_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let values = collect_own_property_values(&mut caller, ptr, true);
            let arr = alloc_array(&mut caller, values.len() as u32);
            let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            for (i, val) in values.iter().enumerate() {
                write_array_elem(&mut caller, arr_ptr, i as u32, *val);
            }
            write_array_length(&mut caller, arr_ptr, values.len() as u32);
            arr
        },
    );
    linker.define(&mut store, "env", "obj_values", obj_values_fn)?;
    // ── Import 87: obj_assign(i64, i64, i32, i32) -> i64 ──────────────────────
    let obj_assign_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         target: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            if !value::is_object(target) && !value::is_function(target) && !value::is_array(target)
            {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: target is not an object".to_string());
                return value::encode_undefined();
            }
            let mut target_ptr = match resolve_handle(&mut caller, target) {
                Some(p) => p,
                None => return target,
            };
            for i in 0..args_count {
                let mut source_val = read_shadow_arg(&mut caller, args_base, i as u32);
                if value::is_undefined(source_val) || value::is_null(source_val) {
                    continue;
                }
                if !value::is_js_object(source_val) {
                    source_val = to_object(&mut caller, source_val);
                }
                let Some(source_ptr) = resolve_handle(&mut caller, source_val) else {
                    continue;
                };
                // 收集源对象的可枚举属性
                let source_props: Vec<(u32, i32, i64)> = {
                    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                        continue;
                    };
                    let d = mem.data(&caller);
                    if source_ptr + 16 > d.len() {
                        continue;
                    }
                    let num_props = u32::from_le_bytes([
                        d[source_ptr + 12],
                        d[source_ptr + 13],
                        d[source_ptr + 14],
                        d[source_ptr + 15],
                    ]) as usize;
                    let mut props = Vec::new();
                    for j in 0..num_props {
                        let slot_offset = source_ptr + 16 + j * 32;
                        if slot_offset + 32 > d.len() {
                            break;
                        }
                        let flags = i32::from_le_bytes([
                            d[slot_offset + 4],
                            d[slot_offset + 5],
                            d[slot_offset + 6],
                            d[slot_offset + 7],
                        ]);
                        if (flags & 2) == 0 {
                            continue;
                        }
                        let nid = u32::from_le_bytes([
                            d[slot_offset],
                            d[slot_offset + 1],
                            d[slot_offset + 2],
                            d[slot_offset + 3],
                        ]);
                        let vl = i64::from_le_bytes([
                            d[slot_offset + 8],
                            d[slot_offset + 9],
                            d[slot_offset + 10],
                            d[slot_offset + 11],
                            d[slot_offset + 12],
                            d[slot_offset + 13],
                            d[slot_offset + 14],
                            d[slot_offset + 15],
                        ]);
                        props.push((nid, flags, vl));
                    }
                    props
                };
                // 写入目标对象 — 先检查容量再写入，避免静默丢弃属性
                // 1) 统计需新增的属性数（源有而目标无）
                let mut new_count: usize = 0;
                for (name_id, _, _) in &source_props {
                    if find_property_slot_by_name_id(&mut caller, target_ptr, *name_id).is_none() {
                        new_count += 1;
                    }
                }
                // 2) 容量不足则扩容（capacity × 2 倍增）
                if new_count > 0 {
                    let need_grow = {
                        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                            continue;
                        };
                        let d = mem.data(&caller);
                        let num = u32::from_le_bytes([
                            d[target_ptr + 12],
                            d[target_ptr + 13],
                            d[target_ptr + 14],
                            d[target_ptr + 15],
                        ]) as usize;
                        let cap = u32::from_le_bytes([
                            d[target_ptr + 8],
                            d[target_ptr + 9],
                            d[target_ptr + 10],
                            d[target_ptr + 11],
                        ]) as usize;
                        num + new_count > cap
                    };
                    if need_grow {
                        let (num, cap) = {
                            let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                                continue;
                            };
                            let d = mem.data(&caller);
                            let n = u32::from_le_bytes([
                                d[target_ptr + 12],
                                d[target_ptr + 13],
                                d[target_ptr + 14],
                                d[target_ptr + 15],
                            ]) as usize;
                            let c = u32::from_le_bytes([
                                d[target_ptr + 8],
                                d[target_ptr + 9],
                                d[target_ptr + 10],
                                d[target_ptr + 11],
                            ]) as usize;
                            (n, c)
                        };
                        let new_cap = (cap * 2).max(num + new_count) as u32;
                        if let Some(new_ptr) = grow_object(&mut caller, target_ptr, target, new_cap)
                        {
                            target_ptr = new_ptr;
                        }
                    }
                }
                // 3) 写入属性（存在则覆盖值，不存在则追加）
                for (name_id, flags, val) in &source_props {
                    let existing = find_property_slot_by_name_id(&mut caller, target_ptr, *name_id);
                    if let Some((existing_offset, _, _)) = existing {
                        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                            continue;
                        };
                        let d = mem.data_mut(&mut caller);
                        d[existing_offset + 8..existing_offset + 16]
                            .copy_from_slice(&val.to_le_bytes());
                    } else {
                        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                            continue;
                        };
                        let d = mem.data_mut(&mut caller);
                        let target_num_props = u32::from_le_bytes([
                            d[target_ptr + 12],
                            d[target_ptr + 13],
                            d[target_ptr + 14],
                            d[target_ptr + 15],
                        ]) as usize;
                        let new_slot_offset = target_ptr + 16 + target_num_props * 32;
                        d[new_slot_offset..new_slot_offset + 4]
                            .copy_from_slice(&name_id.to_le_bytes());
                        d[new_slot_offset + 4..new_slot_offset + 8]
                            .copy_from_slice(&flags.to_le_bytes());
                        d[new_slot_offset + 8..new_slot_offset + 16]
                            .copy_from_slice(&val.to_le_bytes());
                        let zero: u64 = 0;
                        d[new_slot_offset + 16..new_slot_offset + 24]
                            .copy_from_slice(&zero.to_le_bytes());
                        d[new_slot_offset + 24..new_slot_offset + 32]
                            .copy_from_slice(&zero.to_le_bytes());
                        d[target_ptr + 12..target_ptr + 16]
                            .copy_from_slice(&((target_num_props + 1) as u32).to_le_bytes());
                    }
                }
            }
            target
        },
    );
    linker.define(&mut store, "env", "obj_assign", obj_assign_fn)?;
    // ── Import 88: obj_create(i64, i64) -> i64 ────────────────────────────────
    let obj_create_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, proto: i64, _properties: i64| -> i64 {
            let obj_handle = alloc_object(&mut caller, 0);
            if !value::is_null(proto) && !value::is_undefined(proto) {
                // 设置 __proto__：通过内存写 proto 槽位
                let Some(ptr) = resolve_handle(&mut caller, obj_handle) else {
                    return obj_handle;
                };
                let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                    return obj_handle;
                };
                let d = mem.data_mut(&mut caller);
                if value::is_object(proto) || value::is_function(proto) || value::is_array(proto) {
                    let proto_handle = (proto as u64 & 0xFFFF_FFFF) as u32;
                    d[ptr..ptr + 4].copy_from_slice(&proto_handle.to_le_bytes());
                }
            }
            obj_handle
        },
    );
    linker.define(&mut store, "env", "obj_create", obj_create_fn)?;
    // ── Import 90: obj_set_proto_of(i64, i64) -> i64 ──────────────────────────
    let obj_set_proto_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, proto: i64| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) && !value::is_array(obj) {
                return obj; // primitive → no-op per spec
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return obj;
            };
            if value::is_null(proto) || value::is_undefined(proto) {
                // 设置为 null
                let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                    return obj;
                };
                let d = mem.data_mut(&mut caller);
                let null_handle: u32 = 0xFFFF_FFFF;
                d[ptr..ptr + 4].copy_from_slice(&null_handle.to_le_bytes());
                return obj;
            }
            if value::is_object(proto) || value::is_function(proto) || value::is_array(proto) {
                // 循环检测：遍历 proto 的原型链，若 obj 出现在其中则抛出 TypeError
                {
                    let mut current_handle = (proto as u64 & 0xFFFF_FFFF) as u32;
                    let mut depth = 0;
                    const MAX_PROTO_DEPTH: u32 = 1000;
                    let obj_handle = (obj as u64 & 0xFFFF_FFFF) as u32;
                    while current_handle != 0xFFFF_FFFF
                        && current_handle != 0
                        && depth < MAX_PROTO_DEPTH
                    {
                        if current_handle == obj_handle {
                            *caller
                                .data()
                                .runtime_error
                                .lock()
                                .expect("runtime error mutex") =
                                Some("TypeError: Cyclic __proto__ value".to_string());
                            return obj;
                        }
                        let Some(current_ptr) =
                            resolve_handle_idx(&mut caller, current_handle as usize)
                        else {
                            break;
                        };
                        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                            break;
                        };
                        let d = mem.data(&caller);
                        if current_ptr + 4 > d.len() {
                            break;
                        }
                        current_handle = u32::from_le_bytes([
                            d[current_ptr],
                            d[current_ptr + 1],
                            d[current_ptr + 2],
                            d[current_ptr + 3],
                        ]);
                        depth += 1;
                    }
                }
                let proto_handle = (proto as u64 & 0xFFFF_FFFF) as u32;
                let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                    return obj;
                };
                let d = mem.data_mut(&mut caller);
                d[ptr..ptr + 4].copy_from_slice(&proto_handle.to_le_bytes());
            }
            obj
        },
    );
    linker.define(&mut store, "env", "obj_set_proto_of", obj_set_proto_of_fn)?;

    // ── Import: obj_get_own_prop_symbols(i64) -> i64 ────────────────────────
    let obj_get_own_prop_symbols_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let symbols = collect_own_property_key_values(&mut caller, ptr, true);
            let arr = alloc_array(&mut caller, symbols.len() as u32);
            let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            for (i, symbol) in symbols.iter().enumerate() {
                write_array_elem(&mut caller, arr_ptr, i as u32, *symbol);
            }
            write_array_length(&mut caller, arr_ptr, symbols.len() as u32);
            arr
        },
    );
    linker.define(
        &mut store,
        "env",
        "obj_get_own_prop_symbols",
        obj_get_own_prop_symbols_fn,
    )?;
    // ── Import 92: obj_is(i64, i64) -> i64 ────────────────────────────────────
    let obj_is_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, val1: i64, val2: i64| -> i64 {
            // SameValue (ECMAScript 7.2.11)
            // 注意: wjsm 使用 NaN-boxing 编码，NaN-boxed 值的高位与 IEEE NaN 重叠，
            // 必须先区分数值类型再应用 IEEE 754 语义，否则 Object.is(null, undefined) 会错误返回 true
            let bits1 = val1 as u64;
            let bits2 = val2 as u64;
            let is_f64_1 = value::is_f64(val1);
            let is_f64_2 = value::is_f64(val2);
            if is_f64_1 && is_f64_2 {
                // 两者都是 IEEE 754 数值（含 signaling NaN）
                // +0 != -0
                if bits1 == 0 && bits2 == 0x8000_0000_0000_0000 {
                    return value::encode_bool(false);
                }
                if bits1 == 0x8000_0000_0000_0000 && bits2 == 0 {
                    return value::encode_bool(false);
                }
                // NaN == NaN (signaling NaN 区域)
                let f1 = value::decode_f64(val1);
                let f2 = value::decode_f64(val2);
                if f1.is_nan() && f2.is_nan() {
                    return value::encode_bool(true);
                }
                value::encode_bool(bits1 == bits2)
            } else {
                // 至少一个是 NaN-boxed JS 值（或 canonical quiet NaN）
                // NaN-boxed 值用 bitwise 比较：不同 handle/index 表示不同对象
                value::encode_bool(bits1 == bits2)
            }
        },
    );
    linker.define(&mut store, "env", "obj_is", obj_is_fn)?;
    // ── Import 93: obj_proto_to_string(i64) -> i64 ────────────────────────────
    let obj_proto_to_string_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            obj_proto_to_string_impl(&mut caller, obj)
        },
    );
    linker.define(
        &mut store,
        "env",
        "obj_proto_to_string",
        obj_proto_to_string_fn,
    )?;
    // ── Import 94: obj_proto_value_of(i64) -> i64 ─────────────────────────────
    let obj_proto_value_of_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, obj: i64| -> i64 { obj },
    );
    linker.define(
        &mut store,
        "env",
        "obj_proto_value_of",
        obj_proto_value_of_fn,
    )?;
    let obj_proto_init_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let to_string =
                create_native_callable(caller.data(), NativeCallable::ObjectProtoToString);
            let value_of =
                create_native_callable(caller.data(), NativeCallable::ObjectProtoValueOf);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toString", to_string);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "valueOf", value_of);
            value::encode_undefined()
        },
    );
    linker.define(&mut store, "env", "obj_proto_init", obj_proto_init_fn)?;

    // ═══════════════════════════════════════════════════════════════════
    // ── BigInt host functions ──────────────────────────────────────────
    // ═══════════════════════════════════════════════════════════════════

    // ── Import 95: bigint_from_literal(i32, i32) → i64 ─────────────────
    Ok(())
}
