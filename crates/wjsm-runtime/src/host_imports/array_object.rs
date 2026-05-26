use anyhow::Result;
use wasmtime::{Caller, Linker, Func};
use wasmtime::Store;

use crate::*;

pub(crate) fn define_array_object(linker: &mut Linker<RuntimeState>, mut store: &mut Store<RuntimeState>) -> Result<()> {
    let arr_proto_push_fn = Func::wrap(&mut store,
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
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0);
            let count = args_count as u32;
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
    let arr_proto_pop_fn = Func::wrap(&mut store,
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
    let arr_proto_includes_fn = Func::wrap(&mut store,
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
                    && elem == val {
                        return value::encode_bool(true);
                    }
            }
            value::encode_bool(false)
        },
    );
    linker.define(&mut store, "env", "arr_proto_includes", arr_proto_includes_fn)?;

    // ── arr_proto_index_of (#52) ──────────────────────────────────────
    let arr_proto_index_of_fn = Func::wrap(&mut store,
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
                    && elem == val {
                        return value::encode_f64(i as f64);
                    }
            }
            value::encode_f64(-1.0)
        },
    );
    linker.define(&mut store, "env", "arr_proto_index_of", arr_proto_index_of_fn)?;

    // ── arr_proto_join (#53) ─────────────────────────────────────────
    let arr_proto_join_fn = Func::wrap(&mut store,
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
    let arr_proto_concat_fn = Func::wrap(&mut store,
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
            let new_arr = alloc_array(&mut caller, total_len as u32);
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
    let arr_proto_slice_fn = Func::wrap(&mut store,
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
                let s_f64 = f64::from_bits(read_shadow_arg(&mut caller, args_base, 0) as u64);
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
                let e_f64 = f64::from_bits(read_shadow_arg(&mut caller, args_base, 1) as u64);
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
            let new_arr = alloc_array(&mut caller, count);
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
    let arr_proto_fill_fn = Func::wrap(&mut store,
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
                let s_f64 = f64::from_bits(read_shadow_arg(&mut caller, args_base, 1) as u64);
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
                let e_f64 = f64::from_bits(read_shadow_arg(&mut caller, args_base, 2) as u64);
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
    let arr_proto_reverse_fn = Func::wrap(&mut store,
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
    let arr_proto_flat_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            // 读取 depth 参数（默认 1）
            let depth = if args_count > 0 {
                let d = f64::from_bits(read_shadow_arg(&mut caller, args_base, 0) as u64);
                if d.is_nan() { 0 } else { d as u32 }
            } else {
                1
            };
            // 递归展平
            fn flatten(
                caller: &mut Caller<'_, RuntimeState>,
                arr: i64,
                depth: u32,
                result: &mut Vec<i64>,
            ) {
                if depth == 0 {
                    // 不再展平，直接添加数组引用
                    result.push(arr);
                    return;
                }
                let Some(ptr) = resolve_array_ptr(caller, arr) else {
                    result.push(arr);
                    return;
                };
                let len = read_array_length(caller, ptr).unwrap_or(0);
                for i in 0..len {
                    if let Some(elem) = read_array_elem(caller, ptr, i) {
                        if value::is_array(elem) {
                            flatten(caller, elem, depth - 1, result);
                        } else {
                            result.push(elem);
                        }
                    }
                }
            }
            let mut elements = Vec::new();
            flatten(&mut caller, this_val, depth, &mut elements);
            let new_arr = alloc_array(&mut caller, elements.len() as u32);
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
    let arr_proto_shift_fn = Func::wrap(&mut store,
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
    let arr_proto_unshift_fn = Func::wrap(&mut store,
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
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0);
            let new_len = len + args_count as u32;
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

    // ── arr_proto_sort (#61) ──────────────────────────────────────────
    let arr_proto_sort_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return this_val;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as usize;
            if len <= 1 {
                return this_val;
            }
            // 读全部元素到 Vec
            let mut elems: Vec<i64> = (0..len)
                .map(|i| {
                    read_array_elem(&mut caller, ptr, i as u32).unwrap_or(value::encode_undefined())
                })
                .collect();
            if args_count > 0 && value::is_callable(read_shadow_arg(&mut caller, args_base, 0)) {
                let cmp = read_shadow_arg(&mut caller, args_base, 0);
                merge_sort_by(&mut elems, &mut |a, b| -> std::cmp::Ordering {
                    let result =
                        call_wasm_callback(&mut caller, cmp, value::encode_undefined(), &[*a, *b])
                            .unwrap_or(value::encode_f64(0.0));
                    let v = f64::from_bits(result as u64);
                    if v > 0.0 {
                        std::cmp::Ordering::Greater
                    } else if v < 0.0 {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Equal
                    }
                });
            } else {
                let keys: Vec<String> = elems
                    .iter()
                    .map(|e| render_value(&mut caller, *e).unwrap_or_default())
                    .collect();
                // 带原始 index 的稳定排序
                let mut indexed: Vec<(usize, &i64)> = (0..len).map(|i| (i, &elems[i])).collect();
                indexed.sort_by(|(ia, _), (ib, _)| {
                    let ka = &keys[*ia];
                    let kb = &keys[*ib];
                    let cmp = ka.cmp(kb);
                    if cmp == std::cmp::Ordering::Equal {
                        ia.cmp(ib)
                    } else {
                        cmp
                    }
                });
                let sorted: Vec<i64> = indexed.iter().map(|(_, e)| **e).collect();
                elems = sorted;
            }
            // 写回
            for (i, &elem) in elems.iter().enumerate() {
                write_array_elem(&mut caller, ptr, i as u32, elem);
            }
            this_val
        },
    );
    linker.define(&mut store, "env", "arr_proto_sort", arr_proto_sort_fn)?;

    // ── arr_proto_at (#62) ────────────────────────────────────────────
    let arr_proto_at_fn = Func::wrap(&mut store,
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
                let i_f64 = f64::from_bits(read_shadow_arg(&mut caller, args_base, 0) as u64);
                if i_f64.is_nan() {
                    return value::encode_undefined();
                }
                if i_f64 < 0.0 {
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
    let arr_proto_copy_within_fn = Func::wrap(&mut store,
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
                let t = f64::from_bits(read_shadow_arg(&mut caller, args_base, 0) as u64);
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
                let s = f64::from_bits(read_shadow_arg(&mut caller, args_base, 1) as u64);
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
                let e = f64::from_bits(read_shadow_arg(&mut caller, args_base, 2) as u64);
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
            // 复制元素（处理重叠：从后往前复制）
            if target < start {
                for i in 0..count {
                    let elem = read_array_elem(&mut caller, ptr, (start as u32) + i)
                        .unwrap_or(value::encode_undefined());
                    write_array_elem(&mut caller, ptr, (target as u32) + i, elem);
                }
            } else {
                for i in (0..count).rev() {
                    let elem = read_array_elem(&mut caller, ptr, (start as u32) + i)
                        .unwrap_or(value::encode_undefined());
                    write_array_elem(&mut caller, ptr, (target as u32) + i, elem);
                }
            }
            this_val
        },
    );
    linker.define(&mut store, "env", "arr_proto_copy_within", arr_proto_copy_within_fn)?;

    // ── arr_proto_for_each (#64) ─────────────────────────────────────
    let arr_proto_for_each_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                let elem =
                    read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                if call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val])
                    .is_err()
                {
                    return value::encode_undefined();
                }
            }
            value::encode_undefined()
        },
    );
    linker.define(&mut store, "env", "arr_proto_for_each", arr_proto_for_each_fn)?;

    // ── arr_proto_map (#65) ──────────────────────────────────────────
    let arr_proto_map_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let new_arr = alloc_array(&mut caller, len);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for i in 0..len {
                let elem =
                    read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                let result =
                    match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val])
                    {
                        Ok(r) => r,
                        Err(_) => value::encode_undefined(),
                    };
                write_array_elem(&mut caller, new_ptr, i, result);
            }
            write_array_length(&mut caller, new_ptr, len);
            new_arr
        },
    );
    linker.define(&mut store, "env", "arr_proto_map", arr_proto_map_fn)?;

    // ── arr_proto_filter (#66) ───────────────────────────────────────
    let arr_proto_filter_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let mut passed: Vec<i64> = Vec::new();
            for i in 0..len {
                let elem =
                    read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                let ok =
                    match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val])
                    {
                        Ok(r) => value::is_truthy(r),
                        Err(_) => false,
                    };
                if ok {
                    passed.push(elem);
                }
            }
            let new_arr = alloc_array(&mut caller, passed.len() as u32);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for (i, elem) in passed.iter().enumerate() {
                write_array_elem(&mut caller, new_ptr, i as u32, *elem);
            }
            write_array_length(&mut caller, new_ptr, passed.len() as u32);
            new_arr
        },
    );
    linker.define(&mut store, "env", "arr_proto_filter", arr_proto_filter_fn)?;

    // ── arr_proto_reduce (#67) ────────────────────────────────────────
    let arr_proto_reduce_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as usize;
            if len == 0 {
                if args_count < 2 {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime error mutex") =
                        Some("TypeError: Reduce of empty array with no initial value".to_string());
                    return value::encode_undefined();
                }
                return read_shadow_arg(&mut caller, args_base, 1);
            }
            let mut acc: i64;
            let mut start_idx = 0usize;
            if args_count >= 2 {
                acc = read_shadow_arg(&mut caller, args_base, 1);
            } else {
                acc = read_array_elem(&mut caller, ptr, 0).unwrap_or(value::encode_undefined());
                start_idx = 1;
            }
            for i in start_idx..len {
                let elem = read_array_elem(&mut caller, ptr, i as u32)
                    .unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                match call_wasm_callback(
                    &mut caller,
                    cb,
                    value::encode_undefined(),
                    &[acc, elem, idx_val, this_val],
                ) {
                    Ok(r) => acc = r,
                    Err(_) => return value::encode_undefined(),
                }
            }
            acc
        },
    );
    linker.define(&mut store, "env", "arr_proto_reduce", arr_proto_reduce_fn)?;

    // ── arr_proto_reduce_right (#68) ──────────────────────────────────
    let arr_proto_reduce_right_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            if len == 0 {
                if args_count < 2 {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime error mutex") =
                        Some("TypeError: Reduce of empty array with no initial value".to_string());
                    return value::encode_undefined();
                }
                return read_shadow_arg(&mut caller, args_base, 1);
            }
            let mut acc: i64;
            let mut start_idx = len - 1;
            if args_count >= 2 {
                acc = read_shadow_arg(&mut caller, args_base, 1);
            } else {
                acc = read_array_elem(&mut caller, ptr, start_idx as u32)
                    .unwrap_or(value::encode_undefined());
                start_idx = len - 2;
            }
            for i in (0..=start_idx as usize).rev() {
                let elem = read_array_elem(&mut caller, ptr, i as u32)
                    .unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                match call_wasm_callback(
                    &mut caller,
                    cb,
                    value::encode_undefined(),
                    &[acc, elem, idx_val, this_val],
                ) {
                    Ok(r) => acc = r,
                    Err(_) => return value::encode_undefined(),
                }
            }
            acc
        },
    );
    linker.define(&mut store, "env", "arr_proto_reduce_right", arr_proto_reduce_right_fn)?;

    // ── arr_proto_find (#69) ──────────────────────────────────────────
    let arr_proto_find_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         _args_count: i32|
         -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                let elem =
                    read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                if let Ok(r) = call_wasm_callback(
                    &mut caller,
                    cb,
                    value::encode_undefined(),
                    &[elem, idx_val, this_val],
                )
                    && value::is_truthy(r) {
                        return elem;
                    }
            }
            value::encode_undefined()
        },
    );
    linker.define(&mut store, "env", "arr_proto_find", arr_proto_find_fn)?;

    // ── arr_proto_find_index (#70) ────────────────────────────────────
    let arr_proto_find_index_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         _args_count: i32|
         -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_f64(-1.0);
            }
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_f64(-1.0);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                let elem =
                    read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                if let Ok(r) = call_wasm_callback(
                    &mut caller,
                    cb,
                    value::encode_undefined(),
                    &[elem, idx_val, this_val],
                )
                    && value::is_truthy(r) {
                        return value::encode_f64(i as f64);
                    }
            }
            value::encode_f64(-1.0)
        },
    );
    linker.define(&mut store, "env", "arr_proto_find_index", arr_proto_find_index_fn)?;

    // ── arr_proto_some (#71) ─────────────────────────────────────────
    let arr_proto_some_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         _args_count: i32|
         -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_bool(false);
            }
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_bool(false);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                let elem =
                    read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                if let Ok(r) = call_wasm_callback(
                    &mut caller,
                    cb,
                    value::encode_undefined(),
                    &[elem, idx_val, this_val],
                )
                    && value::is_truthy(r) {
                        return value::encode_bool(true);
                    }
            }
            value::encode_bool(false)
        },
    );
    linker.define(&mut store, "env", "arr_proto_some", arr_proto_some_fn)?;

    // ── arr_proto_every (#72) ────────────────────────────────────────
    let arr_proto_every_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         _args_count: i32|
         -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_bool(false);
            }
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_bool(false);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                let elem =
                    read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                match call_wasm_callback(
                    &mut caller,
                    cb,
                    value::encode_undefined(),
                    &[elem, idx_val, this_val],
                ) {
                    Ok(r) => {
                        if !value::is_truthy(r) {
                            return value::encode_bool(false);
                        }
                    }
                    Err(_) => return value::encode_bool(false),
                }
            }
            value::encode_bool(true)
        },
    );
    linker.define(&mut store, "env", "arr_proto_every", arr_proto_every_fn)?;

    // ── arr_proto_flat_map (#73) ─────────────────────────────────────
    let arr_proto_flat_map_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let mut elements: Vec<i64> = Vec::new();
            for i in 0..len {
                let elem =
                    read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                let mapped =
                    match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val])
                    {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                if value::is_array(mapped) {
                    // 展平一层
                    if let Some(mapped_ptr) = resolve_array_ptr(&mut caller, mapped) {
                        let mapped_len = read_array_length(&mut caller, mapped_ptr).unwrap_or(0);
                        for j in 0..mapped_len {
                            if let Some(inner) = read_array_elem(&mut caller, mapped_ptr, j) {
                                elements.push(inner);
                            }
                        }
                    }
                } else {
                    elements.push(mapped);
                }
            }
            let new_arr = alloc_array(&mut caller, elements.len() as u32);
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
    linker.define(&mut store, "env", "arr_proto_flat_map", arr_proto_flat_map_fn)?;

    // ── arr_proto_splice (#74) ───────────────────────────────────────
    let arr_proto_splice_fn = Func::wrap(&mut store,
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
                let s = f64::from_bits(read_shadow_arg(&mut caller, args_base, 0) as u64);
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
                let d = f64::from_bits(read_shadow_arg(&mut caller, args_base, 1) as u64);
                if d.is_nan() { 0 } else { (d as i32).max(0) }
            } else {
                (len - start_idx).max(0)
            };
            let actual_delete = delete_count.min(len - start_idx);
            let insert_count = (args_count - 2).max(0);
            let new_len = len - actual_delete + insert_count;
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
            let deleted_arr = alloc_array(&mut caller, actual_delete as u32);
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
    let arr_proto_is_array_fn = Func::wrap(&mut store,
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
    linker.define(&mut store, "env", "arr_proto_is_array", arr_proto_is_array_fn)?;

    // ── abort_shadow_stack_overflow (#76) ─────────────────────────────
    let abort_shadow_stack_overflow_fn = Func::wrap(&mut store,
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
    linker.define(&mut store, "env", "abort_shadow_stack_overflow", abort_shadow_stack_overflow_fn)?;

    // ── func_call (#78): Function.prototype.call ────────────────────────────
    // 签名: (i64 func, i64 this_val, i64 args_base, i32 args_count) -> i64
    let func_call_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         func: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 { resolve_and_call(&mut caller, func, this_val, args_base, args_count) },
    );
    linker.define(&mut store, "env", "func_call", func_call_fn)?;

    // ── func_apply (#79): Function.prototype.apply ──────────────────────────
    let func_apply_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, func: i64, this_val: i64, args_array: i64| -> i64 {
            func_apply_impl(&mut caller, func, this_val, args_array)
        },
    );
    linker.define(&mut store, "env", "func_apply", func_apply_fn)?;

    // ── func_bind (#80): Function.prototype.bind ────────────────────────────
    let func_bind_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>,
         func: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 { func_bind_impl(&mut caller, func, this_val, args_base, args_count) },
    );
    linker.define(&mut store, "env", "func_bind", func_bind_fn)?;

    // ── object_rest (#81): Exclude specified keys from object ───────────────
    let object_rest_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, excluded_keys: i64| -> i64 {
            object_rest_impl(&mut caller, obj, excluded_keys)
        },
    );
    linker.define(&mut store, "env", "object_rest", object_rest_fn)?;

    // ── obj_spread (#82): Copy own enumerable properties ────────────────────
    let obj_spread_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, dest: i64, source: i64| {
            obj_spread_impl(&mut caller, dest, source);
        },
    );
    linker.define(&mut store, "env", "obj_spread", obj_spread_fn)?;

    // ── Import 83: has_own_property(i64, i32) -> i64 ──────────────────────────
    let has_own_property_fn = Func::wrap(&mut store,
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
    // ── Import 84: obj_keys(i64) -> i64 ───────────────────────────────────────
    let obj_keys_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let names = collect_own_property_names(&mut caller, ptr, true);
            let arr = alloc_array(&mut caller, names.len() as u32);
            let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            for (i, name) in names.iter().enumerate() {
                let key_val = store_runtime_string(&caller, name.clone());
                write_array_elem(&mut caller, arr_ptr, i as u32, key_val);
            }
            write_array_length(&mut caller, arr_ptr, names.len() as u32);
            arr
        },
    );
    linker.define(&mut store, "env", "obj_keys", obj_keys_fn)?;
    // ── Import 85: obj_values(i64) -> i64 ─────────────────────────────────────
    let obj_values_fn = Func::wrap(&mut store,
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
    // ── Import 86: obj_entries(i64) -> i64 ────────────────────────────────────
    let obj_entries_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let names = collect_own_property_names(&mut caller, ptr, true);
            let values = collect_own_property_values(&mut caller, ptr, true);
            let len = names.len().min(values.len());
            let arr = alloc_array(&mut caller, len as u32);
            let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            for i in 0..len {
                // 每个元素是一个 [key, value] 子数组
                let sub_arr = alloc_array(&mut caller, 2);
                let Some(sub_ptr) = resolve_array_ptr(&mut caller, sub_arr) else {
                    continue;
                };
                let key_val = store_runtime_string(&caller, names[i].clone());
                write_array_elem(&mut caller, sub_ptr, 0, key_val);
                write_array_elem(&mut caller, sub_ptr, 1, values[i]);
                write_array_length(&mut caller, sub_ptr, 2);
                write_array_elem(&mut caller, arr_ptr, i as u32, sub_arr);
            }
            write_array_length(&mut caller, arr_ptr, len as u32);
            arr
        },
    );
    linker.define(&mut store, "env", "obj_entries", obj_entries_fn)?;
    // ── Import 87: obj_assign(i64, i64, i32, i32) -> i64 ──────────────────────
    let obj_assign_fn = Func::wrap(&mut store,
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
                let source_val = read_shadow_arg(&mut caller, args_base, i as u32);
                if !value::is_object(source_val)
                    && !value::is_function(source_val)
                    && !value::is_array(source_val)
                {
                    continue;
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
    let obj_create_fn = Func::wrap(&mut store,
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
    // ── Import 89: obj_get_proto_of(i64) -> i64 ───────────────────────────────
    let obj_get_proto_of_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                return value::encode_undefined();
            };
            let d = mem.data(&caller);
            if ptr + 4 > d.len() {
                return value::encode_undefined();
            }
            let proto_handle = u32::from_le_bytes([d[ptr], d[ptr + 1], d[ptr + 2], d[ptr + 3]]);
            if proto_handle == 0xFFFF_FFFF || proto_handle == 0 {
                return value::encode_null();
            }
            let Some(_proto_ptr) = resolve_handle_idx(&mut caller, proto_handle as usize) else {
                return value::encode_null();
            };
            value::encode_object_handle(proto_handle)
        },
    );
    linker.define(&mut store, "env", "obj_get_proto_of", obj_get_proto_of_fn)?;
    // ── Import 90: obj_set_proto_of(i64, i64) -> i64 ──────────────────────────
    let obj_set_proto_of_fn = Func::wrap(&mut store,
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
    // ── Import 91: obj_get_own_prop_names(i64) -> i64 ─────────────────────────
    let obj_get_own_prop_names_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let names = collect_own_property_names(&mut caller, ptr, false);
            let arr = alloc_array(&mut caller, names.len() as u32);
            let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            for (i, name) in names.iter().enumerate() {
                let key_val = store_runtime_string(&caller, name.clone());
                write_array_elem(&mut caller, arr_ptr, i as u32, key_val);
            }
            write_array_length(&mut caller, arr_ptr, names.len() as u32);
            arr
        },
    );
    linker.define(&mut store, "env", "obj_get_own_prop_names", obj_get_own_prop_names_fn)?;
    // ── Import 92: obj_is(i64, i64) -> i64 ────────────────────────────────────
    let obj_is_fn = Func::wrap(&mut store,
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
                let f1 = f64::from_bits(bits1);
                let f2 = f64::from_bits(bits2);
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
    let obj_proto_to_string_fn = Func::wrap(&mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            obj_proto_to_string_impl(&mut caller, obj)
        },
    );
    linker.define(&mut store, "env", "obj_proto_to_string", obj_proto_to_string_fn)?;
    // ── Import 94: obj_proto_value_of(i64) -> i64 ─────────────────────────────
    let obj_proto_value_of_fn = Func::wrap(&mut store,
        |_caller: Caller<'_, RuntimeState>, obj: i64| -> i64 { obj },
    );
    linker.define(&mut store, "env", "obj_proto_value_of", obj_proto_value_of_fn)?;

    // ═══════════════════════════════════════════════════════════════════
    // ── BigInt host functions ──────────────────────────────────────────
    // ═══════════════════════════════════════════════════════════════════

    // ── Import 95: bigint_from_literal(i32, i32) → i64 ─────────────────
    Ok(())
}
