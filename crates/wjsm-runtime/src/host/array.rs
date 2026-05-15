use wasmtime::*;
use wjsm_ir::value;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let arr_push_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0);
            let mut ptr = ptr;
            if len >= cap {
                let new_cap = cap.max(1) * 2;
                let needed = (len + 1).max(new_cap);
                if let Some(new_ptr) = grow_array(&mut caller, ptr, arr, needed) {
                    ptr = new_ptr;
                } else {
                    return value::encode_undefined();
                }
            }
            write_array_elem(&mut caller, ptr, len, val);
            write_array_length(&mut caller, ptr, len + 1);
            value::encode_f64((len + 1) as f64)
        },
    );

    let arr_pop_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
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

    let arr_includes_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_bool(false);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i) {
                    if elem == val {
                        return value::encode_bool(true);
                    }
                }
            }
            value::encode_bool(false)
        },
    );

    let arr_index_of_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64, from_val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_f64(-1.0);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            let from_idx = if value::is_f64(from_val) {
                f64::from_bits(from_val as u64) as i32
            } else {
                0
            };
            let start = if from_idx >= 0 {
                (from_idx as usize).min(len as usize)
            } else {
                ((len + from_idx).max(0)) as usize
            };
            for i in start..len as usize {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i as u32) {
                    if elem == val {
                        return value::encode_f64(i as f64);
                    }
                }
            }
            value::encode_f64(-1.0)
        },
    );

    let arr_join_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, sep_val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let sep_str = render_value(&mut caller, sep_val).unwrap_or_else(|_| ",".to_string());
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

    let arr_concat_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _arr1: i64, _arr2: i64| -> i64 {
            unimplemented!("Array.prototype.concat is not yet implemented in wjsm")
        },
    );

    let arr_slice_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _arr: i64, _start: i64, _end: i64| -> i64 {
            unimplemented!("Array.prototype.slice is not yet implemented in wjsm")
        },
    );

    let arr_fill_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64, _start: i64, _end: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return arr;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                write_array_elem(&mut caller, ptr, i, val);
            }
            arr
        },
    );

    let arr_reverse_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return arr;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len / 2 {
                let a = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let b = read_array_elem(&mut caller, ptr, len - 1 - i)
                    .unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, ptr, i, b);
                write_array_elem(&mut caller, ptr, len - 1 - i, a);
            }
            arr
        },
    );

    let arr_flat_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _arr: i64, _depth: i64| -> i64 {
            unimplemented!("Array.prototype.flat is not yet implemented in wjsm")
        },
    );

    let arr_init_length_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, len_val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return arr;
            };
            let len = if value::is_f64(len_val) {
                f64::from_bits(len_val as u64) as u32
            } else {
                return arr;
            };
            write_array_length(&mut caller, ptr, len);
            arr
        },
    );

    let arr_get_length_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            value::encode_f64(len as f64)
        },
    );

    // ── 辅助函数：读取影子栈参数 ────────────────────────────────────

    let arr_proto_push_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_pop (#50) ───────────────────────────────────────────

    let arr_proto_pop_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_includes (#51) ──────────────────────────────────────

    let arr_proto_includes_fn = Func::wrap(
        &mut *store,
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
                if let Some(elem) = read_array_elem(&mut caller, ptr, i) {
                    if elem == val {
                        return value::encode_bool(true);
                    }
                }
            }
            value::encode_bool(false)
        },
    );

    // ── arr_proto_index_of (#52) ──────────────────────────────────────

    let arr_proto_index_of_fn = Func::wrap(
        &mut *store,
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
                if let Some(elem) = read_array_elem(&mut caller, ptr, i) {
                    if elem == val {
                        return value::encode_f64(i as f64);
                    }
                }
            }
            value::encode_f64(-1.0)
        },
    );

    // ── arr_proto_join (#53) ─────────────────────────────────────────

    let arr_proto_join_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_concat (#54) ────────────────────────────────────────

    let arr_proto_concat_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_slice (#55) ─────────────────────────────────────────

    let arr_proto_slice_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_fill (#56) ──────────────────────────────────────────

    let arr_proto_fill_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_reverse (#57) ───────────────────────────────────────

    let arr_proto_reverse_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_flat (#58) ──────────────────────────────────────────

    let arr_proto_flat_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_shift (#59) ─────────────────────────────────────────

    let arr_proto_shift_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_unshift (#60) ───────────────────────────────────────

    let arr_proto_unshift_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_sort (#61) ──────────────────────────────────────────

    let arr_proto_sort_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_at (#62) ────────────────────────────────────────────

    let arr_proto_at_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_copy_within (#63) ──────────────────────────────────

    let arr_proto_copy_within_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_for_each (#64) ─────────────────────────────────────

    let arr_proto_for_each_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_map (#65) ──────────────────────────────────────────

    let arr_proto_map_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_filter (#66) ───────────────────────────────────────

    let arr_proto_filter_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_reduce (#67) ────────────────────────────────────────

    let arr_proto_reduce_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_reduce_right (#68) ──────────────────────────────────

    let arr_proto_reduce_right_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_find (#69) ──────────────────────────────────────────

    let arr_proto_find_fn = Func::wrap(
        &mut *store,
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
                match call_wasm_callback(
                    &mut caller,
                    cb,
                    value::encode_undefined(),
                    &[elem, idx_val, this_val],
                ) {
                    Ok(r) => {
                        if value::is_truthy(r) {
                            return elem;
                        }
                    }
                    Err(_) => {}
                }
            }
            value::encode_undefined()
        },
    );

    // ── arr_proto_find_index (#70) ────────────────────────────────────

    let arr_proto_find_index_fn = Func::wrap(
        &mut *store,
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
                match call_wasm_callback(
                    &mut caller,
                    cb,
                    value::encode_undefined(),
                    &[elem, idx_val, this_val],
                ) {
                    Ok(r) => {
                        if value::is_truthy(r) {
                            return value::encode_f64(i as f64);
                        }
                    }
                    Err(_) => {}
                }
            }
            value::encode_f64(-1.0)
        },
    );

    // ── arr_proto_some (#71) ─────────────────────────────────────────

    let arr_proto_some_fn = Func::wrap(
        &mut *store,
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
                        if value::is_truthy(r) {
                            return value::encode_bool(true);
                        }
                    }
                    Err(_) => {}
                }
            }
            value::encode_bool(false)
        },
    );

    // ── arr_proto_every (#72) ────────────────────────────────────────

    let arr_proto_every_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_flat_map (#73) ─────────────────────────────────────

    let arr_proto_flat_map_fn = Func::wrap(
        &mut *store,
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

    // ── arr_proto_splice (#74) ───────────────────────────────────────

    let arr_proto_splice_fn = Func::wrap(
        &mut *store,
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
            let insert_count = (args_count - 2).max(0) as i32;
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

    // ── arr_proto_is_array (#75) ──────────────────────────────────────

    let arr_proto_is_array_fn = Func::wrap(
        &mut *store,
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

    // ── abort_shadow_stack_overflow (#76) ─────────────────────────────

    vec![
        (38, arr_push_fn),
        (39, arr_pop_fn),
        (40, arr_includes_fn),
        (41, arr_index_of_fn),
        (42, arr_join_fn),
        (43, arr_concat_fn),
        (44, arr_slice_fn),
        (45, arr_fill_fn),
        (46, arr_reverse_fn),
        (47, arr_flat_fn),
        (48, arr_init_length_fn),
        (49, arr_get_length_fn),
        (50, arr_proto_push_fn),
        (51, arr_proto_pop_fn),
        (52, arr_proto_includes_fn),
        (53, arr_proto_index_of_fn),
        (54, arr_proto_join_fn),
        (55, arr_proto_concat_fn),
        (56, arr_proto_slice_fn),
        (57, arr_proto_fill_fn),
        (58, arr_proto_reverse_fn),
        (59, arr_proto_flat_fn),
        (60, arr_proto_shift_fn),
        (61, arr_proto_unshift_fn),
        (62, arr_proto_sort_fn),
        (63, arr_proto_at_fn),
        (64, arr_proto_copy_within_fn),
        (65, arr_proto_for_each_fn),
        (66, arr_proto_map_fn),
        (67, arr_proto_filter_fn),
        (68, arr_proto_reduce_fn),
        (69, arr_proto_reduce_right_fn),
        (70, arr_proto_find_fn),
        (71, arr_proto_find_index_fn),
        (72, arr_proto_some_fn),
        (73, arr_proto_every_fn),
        (74, arr_proto_flat_map_fn),
        (75, arr_proto_splice_fn),
        (76, arr_proto_is_array_fn),
    ]
}
