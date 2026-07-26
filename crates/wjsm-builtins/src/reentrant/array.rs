//! Array.prototype 再入型方法（map/filter/reduce/sort 等）。
//!
//! 回调经 `call_js_async`；ArraySpeciesCreate 经 ExecContext 原语。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

async fn sort_compare_async<E: ExecContext>(
    ctx: &mut E,
    cmp: Value,
    a: Value,
    b: Value,
) -> std::cmp::Ordering {
    let result = ctx
        .call_js_async(cmp, value::encode_undefined(), &[a, b])
        .await
        .unwrap_or_else(|_| value::encode_f64(0.0));
    let v = value::decode_f64(result);
    if v > 0.0 {
        std::cmp::Ordering::Greater
    } else if v < 0.0 {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Equal
    }
}

/// SortCompareList 条目：仅包含参与排序的已定义且非 undefined 元素。
struct SortableElem {
    value: Value,
    original_index: u32,
}

fn array_get_or_undefined<E: ExecContext>(ctx: &mut E, arr: Value, i: u32) -> Value {
    ctx.array_elem_at(arr, i)
        .unwrap_or_else(value::encode_undefined)
}

fn array_present<E: ExecContext>(ctx: &mut E, arr: Value, i: u32) -> bool {
    ctx.array_elem_at(arr, i).is_some()
}

fn read_cb_and_this<E: ExecContext>(
    ctx: &mut E,
    args_base: i32,
    args_count: i32,
) -> Option<(Value, Value)> {
    let cb = ctx.read_shadow_arg(args_base, 0);
    if !ctx.is_callable(cb) {
        return None;
    }
    let this_arg = if args_count > 1 {
        ctx.read_shadow_arg(args_base, 1)
    } else {
        value::encode_undefined()
    };
    Some((cb, this_arg))
}

/// ECMAScript Array.prototype.sort
pub async fn arr_proto_sort<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if !ctx.resolve_array(this_val) {
        return this_val;
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    if len <= 1 {
        return this_val;
    }

    let mut sort_list: Vec<SortableElem> = Vec::new();
    let mut undefined_count: u32 = 0;
    let mut hole_count: u32 = 0;

    for i in 0..len {
        if !array_present(ctx, this_val, i) {
            hole_count += 1;
            continue;
        }
        let elem = array_get_or_undefined(ctx, this_val, i);
        if value::is_undefined(elem) {
            undefined_count += 1;
        } else {
            sort_list.push(SortableElem {
                value: elem,
                original_index: i,
            });
        }
    }

    if !sort_list.is_empty() {
        let has_cmp = args_count > 0 && {
            let c = ctx.read_shadow_arg(args_base, 0);
            ctx.is_callable(c)
        };
        if has_cmp {
            let cmp = ctx.read_shadow_arg(args_base, 0);
            for i in 0..sort_list.len() {
                for j in i + 1..sort_list.len() {
                    if sort_compare_async(ctx, cmp, sort_list[i].value, sort_list[j].value).await
                        == std::cmp::Ordering::Greater
                    {
                        sort_list.swap(i, j);
                    }
                }
            }
        } else {
            let mut keys = Vec::with_capacity(sort_list.len());
            for e in &sort_list {
                keys.push(ctx.render_value(e.value));
            }
            let mut order: Vec<usize> = (0..sort_list.len()).collect();
            order.sort_by(|&ia, &ib| {
                let ord = keys[ia].cmp(&keys[ib]);
                if ord == std::cmp::Ordering::Equal {
                    sort_list[ia]
                        .original_index
                        .cmp(&sort_list[ib].original_index)
                } else {
                    ord
                }
            });
            sort_list = order
                .into_iter()
                .map(|i| SortableElem {
                    value: sort_list[i].value,
                    original_index: sort_list[i].original_index,
                })
                .collect();
        }
    }

    let mut write_idx: u32 = 0;
    for item in &sort_list {
        ctx.array_write_elem(this_val, write_idx, item.value);
        write_idx += 1;
    }
    for _ in 0..undefined_count {
        ctx.array_write_elem(this_val, write_idx, value::encode_undefined());
        write_idx += 1;
    }
    for _ in 0..hole_count {
        ctx.array_write_hole(this_val, write_idx);
        write_idx += 1;
    }

    this_val
}

/// Array.prototype.forEach
pub async fn arr_proto_for_each<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some((cb, this_arg)) = read_cb_and_this(ctx, args_base, args_count) else {
        return value::encode_undefined();
    };
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    for i in 0..len {
        if !array_present(ctx, this_val, i) {
            continue;
        }
        let elem = array_get_or_undefined(ctx, this_val, i);
        let idx_val = value::encode_f64(i as f64);
        if ctx
            .call_js_async(cb, this_arg, &[elem, idx_val, this_val])
            .await
            .is_err()
        {
            return value::encode_undefined();
        }
    }
    value::encode_undefined()
}

/// Array.prototype.map
pub async fn arr_proto_map<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some((cb, this_arg)) = read_cb_and_this(ctx, args_base, args_count) else {
        return value::encode_undefined();
    };
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    let new_arr = ctx.array_species_create_async(this_val, len).await;
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    let temp_root_len = ctx.push_temp_roots(&[new_arr]);
    for i in 0..len {
        if !ctx.resolve_array(this_val) {
            break;
        }
        if !array_present(ctx, this_val, i) {
            if ctx.resolve_array(new_arr) {
                ctx.array_write_hole(new_arr, i);
                ctx.array_write_length(new_arr, i + 1);
            }
            continue;
        }
        let elem = array_get_or_undefined(ctx, this_val, i);
        let idx_val = value::encode_f64(i as f64);
        let result = ctx
            .call_js_async(cb, this_arg, &[elem, idx_val, this_val])
            .await
            .unwrap_or_else(|_| value::encode_undefined());
        if ctx.resolve_array(new_arr) {
            ctx.array_write_elem(new_arr, i, result);
            ctx.array_write_length(new_arr, i + 1);
        }
    }
    if ctx.resolve_array(new_arr) {
        ctx.array_write_length(new_arr, len);
    }
    ctx.truncate_temp_roots(temp_root_len);
    new_arr
}

/// Array.prototype.filter
pub async fn arr_proto_filter<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some((cb, this_arg)) = read_cb_and_this(ctx, args_base, args_count) else {
        return value::encode_undefined();
    };
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    let mut passed: Vec<Value> = Vec::new();
    for i in 0..len {
        if !array_present(ctx, this_val, i) {
            continue;
        }
        let elem = array_get_or_undefined(ctx, this_val, i);
        let idx_val = value::encode_f64(i as f64);
        let ok = match ctx
            .call_js_async(cb, this_arg, &[elem, idx_val, this_val])
            .await
        {
            Ok(r) => value::is_truthy(r),
            Err(_) => false,
        };
        if ok {
            passed.push(elem);
        }
    }
    let new_arr = ctx
        .array_species_create_async(this_val, passed.len() as u32)
        .await;
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    for (i, elem) in passed.iter().enumerate() {
        ctx.array_write_elem(new_arr, i as u32, *elem);
    }
    ctx.array_write_length(new_arr, passed.len() as u32);
    new_arr
}

/// Array.prototype.reduce
pub async fn arr_proto_reduce<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let cb = ctx.read_shadow_arg(args_base, 0);
    if !ctx.is_callable(cb) {
        return value::encode_undefined();
    }
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0) as usize;
    if len == 0 {
        if args_count < 2 {
            ctx.set_last_error(
                "TypeError: Reduce of empty array with no initial value".to_string(),
            );
            return value::encode_undefined();
        }
        return ctx.read_shadow_arg(args_base, 1);
    }
    let mut acc: Value;
    let mut start_idx = 0usize;
    if args_count >= 2 {
        acc = ctx.read_shadow_arg(args_base, 1);
    } else {
        acc = array_get_or_undefined(ctx, this_val, 0);
        start_idx = 1;
    }
    for i in start_idx..len {
        let elem = array_get_or_undefined(ctx, this_val, i as u32);
        let idx_val = value::encode_f64(i as f64);
        match ctx
            .call_js_async(cb, value::encode_undefined(), &[acc, elem, idx_val, this_val])
            .await
        {
            Ok(r) => acc = r,
            Err(_) => return value::encode_undefined(),
        }
    }
    acc
}

/// Array.prototype.reduceRight
pub async fn arr_proto_reduce_right<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let cb = ctx.read_shadow_arg(args_base, 0);
    if !ctx.is_callable(cb) {
        return value::encode_undefined();
    }
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0) as i32;
    if len == 0 {
        if args_count < 2 {
            ctx.set_last_error(
                "TypeError: Reduce of empty array with no initial value".to_string(),
            );
            return value::encode_undefined();
        }
        return ctx.read_shadow_arg(args_base, 1);
    }
    let mut acc: Value;
    let mut start_idx = len - 1;
    if args_count >= 2 {
        acc = ctx.read_shadow_arg(args_base, 1);
    } else {
        acc = array_get_or_undefined(ctx, this_val, start_idx as u32);
        start_idx = len - 2;
    }
    for i in (0..=start_idx as usize).rev() {
        let elem = array_get_or_undefined(ctx, this_val, i as u32);
        let idx_val = value::encode_f64(i as f64);
        match ctx
            .call_js_async(cb, value::encode_undefined(), &[acc, elem, idx_val, this_val])
            .await
        {
            Ok(r) => acc = r,
            Err(_) => return value::encode_undefined(),
        }
    }
    acc
}

/// Array.prototype.find
pub async fn arr_proto_find<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    _args_count: i32,
) -> Value {
    let cb = ctx.read_shadow_arg(args_base, 0);
    if !ctx.is_callable(cb) {
        return value::encode_undefined();
    }
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    for i in 0..len {
        let elem = array_get_or_undefined(ctx, this_val, i);
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = ctx
            .call_js_async(cb, value::encode_undefined(), &[elem, idx_val, this_val])
            .await
            && value::is_truthy(r)
        {
            return elem;
        }
    }
    value::encode_undefined()
}

/// Array.prototype.findIndex
pub async fn arr_proto_find_index<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    _args_count: i32,
) -> Value {
    let cb = ctx.read_shadow_arg(args_base, 0);
    if !ctx.is_callable(cb) {
        return value::encode_f64(-1.0);
    }
    if !ctx.resolve_array(this_val) {
        return value::encode_f64(-1.0);
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    for i in 0..len {
        let elem = array_get_or_undefined(ctx, this_val, i);
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = ctx
            .call_js_async(cb, value::encode_undefined(), &[elem, idx_val, this_val])
            .await
            && value::is_truthy(r)
        {
            return value::encode_f64(i as f64);
        }
    }
    value::encode_f64(-1.0)
}

/// Array.prototype.some
pub async fn arr_proto_some<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    _args_count: i32,
) -> Value {
    let cb = ctx.read_shadow_arg(args_base, 0);
    if !ctx.is_callable(cb) {
        return value::encode_bool(false);
    }
    if !ctx.resolve_array(this_val) {
        return value::encode_bool(false);
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    for i in 0..len {
        let elem = array_get_or_undefined(ctx, this_val, i);
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = ctx
            .call_js_async(cb, value::encode_undefined(), &[elem, idx_val, this_val])
            .await
            && value::is_truthy(r)
        {
            return value::encode_bool(true);
        }
    }
    value::encode_bool(false)
}

/// Array.prototype.every
pub async fn arr_proto_every<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    _args_count: i32,
) -> Value {
    let cb = ctx.read_shadow_arg(args_base, 0);
    if !ctx.is_callable(cb) {
        return value::encode_bool(false);
    }
    if !ctx.resolve_array(this_val) {
        return value::encode_bool(false);
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    for i in 0..len {
        let elem = array_get_or_undefined(ctx, this_val, i);
        let idx_val = value::encode_f64(i as f64);
        match ctx
            .call_js_async(cb, value::encode_undefined(), &[elem, idx_val, this_val])
            .await
        {
            Ok(r) => {
                if !value::is_truthy(r) {
                    return value::encode_bool(false);
                }
            }
            Err(_) => return value::encode_bool(false),
        }
    }
    value::encode_bool(true)
}

/// Array.prototype.flatMap
pub async fn arr_proto_flat_map<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some((cb, this_arg)) = read_cb_and_this(ctx, args_base, args_count) else {
        return value::encode_undefined();
    };
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    let mut elements: Vec<Value> = Vec::new();
    for i in 0..len {
        let elem = array_get_or_undefined(ctx, this_val, i);
        let idx_val = value::encode_f64(i as f64);
        let mapped = match ctx
            .call_js_async(cb, this_arg, &[elem, idx_val, this_val])
            .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        if value::is_array(mapped) {
            let mapped_len = ctx.array_read_length(mapped).unwrap_or(0);
            for j in 0..mapped_len {
                if let Some(inner) = ctx.array_elem_at(mapped, j) {
                    elements.push(inner);
                }
            }
        } else {
            elements.push(mapped);
        }
    }
    let new_arr = ctx
        .array_species_create_async(this_val, elements.len() as u32)
        .await;
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    for (i, elem) in elements.iter().enumerate() {
        ctx.array_write_elem(new_arr, i as u32, *elem);
    }
    ctx.array_write_length(new_arr, elements.len() as u32);
    new_arr
}

/// Array.prototype.findLast
pub async fn arr_proto_find_last<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    _args_count: i32,
) -> Value {
    let cb = ctx.read_shadow_arg(args_base, 0);
    if !ctx.is_callable(cb) {
        return value::encode_undefined();
    }
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    for i in (0..len).rev() {
        let elem = array_get_or_undefined(ctx, this_val, i);
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = ctx
            .call_js_async(cb, value::encode_undefined(), &[elem, idx_val, this_val])
            .await
            && value::is_truthy(r)
        {
            return elem;
        }
    }
    value::encode_undefined()
}

/// Array.prototype.findLastIndex
pub async fn arr_proto_find_last_index<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    _args_count: i32,
) -> Value {
    let cb = ctx.read_shadow_arg(args_base, 0);
    if !ctx.is_callable(cb) {
        return value::encode_f64(-1.0);
    }
    if !ctx.resolve_array(this_val) {
        return value::encode_f64(-1.0);
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    for i in (0..len).rev() {
        let elem = array_get_or_undefined(ctx, this_val, i);
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = ctx
            .call_js_async(cb, value::encode_undefined(), &[elem, idx_val, this_val])
            .await
            && value::is_truthy(r)
        {
            return value::encode_f64(i as f64);
        }
    }
    value::encode_f64(-1.0)
}

/// Array.prototype.toSorted
pub async fn arr_proto_to_sorted<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let has_comparator = args_count > 0 && {
        let c = ctx.read_shadow_arg(args_base, 0);
        ctx.is_callable(c)
    };
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    let new_arr = ctx.alloc_array(len);
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    if len == 0 {
        ctx.array_write_length(new_arr, 0);
        return new_arr;
    }

    let mut sort_list: Vec<Value> = Vec::new();
    let mut undefined_count: u32 = 0;
    for i in 0..len {
        let elem = array_get_or_undefined(ctx, this_val, i);
        if value::is_undefined(elem) {
            undefined_count += 1;
        } else {
            sort_list.push(elem);
        }
    }

    if sort_list.len() > 1 {
        if has_comparator {
            let cmp = ctx.read_shadow_arg(args_base, 0);
            for i in 0..sort_list.len() {
                for j in i + 1..sort_list.len() {
                    if sort_compare_async(ctx, cmp, sort_list[i], sort_list[j]).await
                        == std::cmp::Ordering::Greater
                    {
                        sort_list.swap(i, j);
                    }
                }
            }
        } else {
            let mut keys = Vec::with_capacity(sort_list.len());
            for &e in &sort_list {
                keys.push(ctx.render_value(e));
            }
            let mut order: Vec<usize> = (0..sort_list.len()).collect();
            order.sort_by(|&ia, &ib| keys[ia].cmp(&keys[ib]));
            sort_list = order.into_iter().map(|i| sort_list[i]).collect();
        }
    }

    let mut write_idx: u32 = 0;
    for &v in &sort_list {
        ctx.array_write_elem(new_arr, write_idx, v);
        write_idx += 1;
    }
    for _ in 0..undefined_count {
        ctx.array_write_elem(new_arr, write_idx, value::encode_undefined());
        write_idx += 1;
    }
    ctx.array_write_length(new_arr, len);
    new_arr
}

/// Function.prototype.call（再入）
pub async fn func_call<E: ExecContext>(
    ctx: &mut E,
    func: Value,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let mut args = Vec::with_capacity(args_count.max(0) as usize);
    for i in 0..args_count.max(0) as u32 {
        args.push(ctx.read_shadow_arg(args_base, i));
    }
    ctx.call_js_async(func, this_val, &args)
        .await
        .unwrap_or_else(|_| value::encode_undefined())
}

/// Function.prototype.apply（再入）
pub async fn func_apply<E: ExecContext>(
    ctx: &mut E,
    func: Value,
    this_val: Value,
    args_array: Value,
) -> Value {
    let args = extract_array_like_elements(ctx, args_array);
    ctx.call_js_async(func, this_val, &args)
        .await
        .unwrap_or_else(|_| value::encode_undefined())
}

fn extract_array_like_elements<E: ExecContext>(ctx: &mut E, args_array: Value) -> Vec<Value> {
    if value::is_array(args_array) {
        let len = ctx.array_read_length(args_array).unwrap_or(0);
        let mut out = Vec::with_capacity(len as usize);
        for i in 0..len {
            out.push(array_get_or_undefined(ctx, args_array, i));
        }
        return out;
    }
    if value::is_object(args_array) {
        let len_val = ctx.read_property_by_string_key(args_array, "length");
        let n = value::decode_f64(ctx.to_number(len_val));
        if !n.is_finite() || n <= 0.0 {
            return Vec::new();
        }
        let len = n.trunc().min(u32::MAX as f64) as u32;
        let mut out = Vec::with_capacity(len as usize);
        for i in 0..len {
            let key = i.to_string();
            out.push(ctx.read_property_by_string_key(args_array, &key));
        }
        return out;
    }
    Vec::new()
}

/// Array 展开 push（`arr.push(...iterable)` / `[...iterable]`）
///
/// 协议：对同步侧表迭代器（String/Array/…）先判定 done → 读 current → 再 advance，
/// 与 for-of / `drain_raw_iterator_values` 一致；禁止 advance-first（会丢掉首元素）。
pub async fn array_push_spread<E: ExecContext>(
    ctx: &mut E,
    arr: Value,
    iterable: Value,
) -> Value {
    if value::is_array(iterable) {
        let len = ctx.array_read_length(iterable).unwrap_or(0);
        for i in 0..len {
            let val = array_get_or_undefined(ctx, iterable, i);
            let _ = ctx.array_push(arr, val);
        }
        return arr;
    }
    // 字符串快速路径：按 UTF-16 码点展开（与 host 原实现一致）
    if value::is_string(iterable) {
        let string = ctx.get_runtime_string(iterable);
        let mut unit_pos = 0usize;
        while unit_pos < string.utf16_len() {
            let Some(unit) = string.code_unit_at(unit_pos) else {
                break;
            };
            let width = if (0xD800..=0xDBFF).contains(&unit)
                && string
                    .code_unit_at(unit_pos + 1)
                    .is_some_and(|next| (0xDC00..=0xDFFF).contains(&next))
            {
                2
            } else {
                1
            };
            let slice = string.slice_units(unit_pos..unit_pos + width);
            let val = ctx.store_runtime_string(slice);
            let _ = ctx.array_push(arr, val);
            unit_pos += width;
        }
        return arr;
    }
    // 通用迭代器路径：GetIterator + next 循环
    let iterator = match ctx.iterator_from_fallback_async(iterable).await {
        it if !value::is_undefined(it) && !value::is_null(it) => it,
        _ => {
            ctx.set_last_error("TypeError: object is not iterable".to_string());
            return value::encode_undefined();
        }
    };
    loop {
        match ctx.iterator_done_sync(iterator) {
            Some(true) => break,
            Some(false) => {
                // 同步侧表迭代器：current 已就绪（for-of 同序）
                let val = ctx.iterator_current_value(iterator);
                let _ = ctx.array_push(arr, val);
                // 推进；ObjectIter 在 has_current 被消费后会走到 NeedObjectNext
                match ctx.iterator_next_sync_step(iterator) {
                    wjsm_host::IteratorNextStep::Advanced
                    | wjsm_host::IteratorNextStep::Missing
                    | wjsm_host::IteratorNextStep::ErrorDone => {}
                    wjsm_host::IteratorNextStep::NeedObjectNext { iterator: it, next } => {
                        if !spread_push_object_next(ctx, arr, iterator, it, next).await {
                            break;
                        }
                    }
                    wjsm_host::IteratorNextStep::NeedAsyncFromSync { afs } => {
                        if !spread_push_afs_next(ctx, arr, afs).await {
                            break;
                        }
                    }
                }
            }
            None => {
                // ObjectIter 尚无 current：调用 next()
                match ctx.iterator_next_sync_step(iterator) {
                    wjsm_host::IteratorNextStep::NeedObjectNext { iterator: it, next } => {
                        if !spread_push_object_next(ctx, arr, iterator, it, next).await {
                            break;
                        }
                    }
                    wjsm_host::IteratorNextStep::NeedAsyncFromSync { afs } => {
                        if !spread_push_afs_next(ctx, arr, afs).await {
                            break;
                        }
                    }
                    wjsm_host::IteratorNextStep::Advanced => {
                        if ctx.iterator_done_sync(iterator) == Some(true) {
                            break;
                        }
                        let val = ctx.iterator_current_value(iterator);
                        let _ = ctx.array_push(arr, val);
                    }
                    wjsm_host::IteratorNextStep::Missing
                    | wjsm_host::IteratorNextStep::ErrorDone => break,
                }
            }
        }
    }
    arr
}

/// ObjectIter：调用 `next`；未 done 则 push，并清空 has_current 以便下次再 next。
async fn spread_push_object_next<E: ExecContext>(
    ctx: &mut E,
    arr: Value,
    handle: Value,
    iterator: Value,
    next: Value,
) -> bool {
    let result = match ctx.call_js_async(next, iterator, &[]).await {
        Ok(r) => r,
        Err(_) => return false,
    };
    if value::is_exception(result) {
        ctx.set_last_error("TypeError: iterator.next() threw".to_string());
        return false;
    }
    let Some((val, done)) = ctx.parse_iterator_result(result) else {
        return false;
    };
    if done {
        ctx.iterator_store_object_current(handle, value::encode_undefined(), true, false);
        return false;
    }
    let _ = ctx.array_push(arr, val);
    // 清空 has_current，避免下一轮 done_sync=Some(false) 重复 yield 同一值
    ctx.iterator_store_object_current(handle, value::encode_undefined(), false, false);
    true
}

async fn spread_push_afs_next<E: ExecContext>(ctx: &mut E, arr: Value, afs: u32) -> bool {
    let result = ctx.iterator_materialize_afs_next(afs).await;
    if value::is_exception(result) {
        return false;
    }
    let Some((val, done)) = ctx.parse_iterator_result(result) else {
        return false;
    };
    if done {
        return false;
    }
    let _ = ctx.array_push(arr, val);
    true
}
