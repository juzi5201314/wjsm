//! TypedArray.prototype 再入型方法（map/filter/reduce/sort 等）。
//!
//! 元素读写经 `typedarray_read_elem` / `typedarray_write_elem`；回调经 `call_js`。

use wjsm_host::{ExecContext, TypedArrayView, Value};
use wjsm_ir::value;

fn sort_compare<E: ExecContext>(ctx: &mut E, cmp: Value, a: Value, b: Value) -> std::cmp::Ordering {
    let result = ctx
        .call_js(cmp, value::encode_undefined(), &[a, b])
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

fn ta_elem<E: ExecContext>(ctx: &mut E, view: &TypedArrayView, i: u32) -> Value {
    ctx.typedarray_read_elem(view, i)
        .unwrap_or_else(value::encode_undefined)
}

fn read_cb_and_this<E: ExecContext>(
    ctx: &mut E,
    args_base: i32,
    args_count: i32,
) -> Option<(Value, Value)> {
    let cb = ctx.read_call_arg(
        wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
        0,
    );
    if !ctx.is_callable(cb) {
        return None;
    }
    let this_arg = if args_count > 1 {
        ctx.read_call_arg(
            wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
            1,
        )
    } else {
        value::encode_undefined()
    };
    Some((cb, this_arg))
}

/// TypedArray.prototype.sort
pub fn typedarray_proto_sort<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return this_val;
    };
    if view.length <= 1 {
        return this_val;
    }
    let mut elems: Vec<Value> = Vec::with_capacity(view.length as usize);
    for i in 0..view.length {
        elems.push(ta_elem(ctx, &view, i));
    }

    let has_cmp = args_count > 0 && {
        let c = ctx.read_call_arg(
            wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
            0,
        );
        ctx.is_callable(c)
    };
    if has_cmp {
        let cmp = ctx.read_call_arg(
            wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
            0,
        );
        for i in 0..elems.len() {
            for j in i + 1..elems.len() {
                if sort_compare(ctx, cmp, elems[i], elems[j]) == std::cmp::Ordering::Greater {
                    elems.swap(i, j);
                }
            }
        }
    } else {
        let mut keys = Vec::with_capacity(elems.len());
        for &e in &elems {
            keys.push(ctx.render_value(e));
        }
        let mut indexed: Vec<usize> = (0..elems.len()).collect();
        indexed.sort_by(|&ia, &ib| {
            let cmp = keys[ia].cmp(&keys[ib]);
            if cmp == std::cmp::Ordering::Equal {
                ia.cmp(&ib)
            } else {
                cmp
            }
        });
        elems = indexed.into_iter().map(|i| elems[i]).collect();
    }
    for (i, &elem) in elems.iter().enumerate() {
        ctx.typedarray_write_elem(&view, i as u32, elem);
    }
    this_val
}

/// TypedArray.prototype.forEach
pub fn typedarray_proto_for_each<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_undefined();
    };
    let Some((cb, this_arg)) = read_cb_and_this(ctx, args_base, args_count) else {
        return value::encode_undefined();
    };
    for i in 0..view.length {
        let elem = ta_elem(ctx, &view, i);
        let idx_val = value::encode_f64(i as f64);
        if ctx
            .call_js(cb, this_arg, &[elem, idx_val, this_val])
            .is_err()
        {
            return value::encode_undefined();
        }
    }
    value::encode_undefined()
}

/// TypedArray.prototype.map — 返回普通 Array（与现有 host 语义一致）
pub fn typedarray_proto_map<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_undefined();
    };
    let Some((cb, this_arg)) = read_cb_and_this(ctx, args_base, args_count) else {
        return value::encode_undefined();
    };
    let new_arr = ctx.alloc_array(view.length);
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    for i in 0..view.length {
        let elem = ta_elem(ctx, &view, i);
        let idx_val = value::encode_f64(i as f64);
        let mapped = match ctx.call_js(cb, this_arg, &[elem, idx_val, this_val]) {
            Ok(v) => v,
            Err(_) => return value::encode_undefined(),
        };
        ctx.array_write_elem(new_arr, i, mapped);
    }
    ctx.array_write_length(new_arr, view.length);
    new_arr
}

/// TypedArray.prototype.filter
pub fn typedarray_proto_filter<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_undefined();
    };
    let Some((cb, this_arg)) = read_cb_and_this(ctx, args_base, args_count) else {
        return value::encode_undefined();
    };
    let mut results = Vec::new();
    for i in 0..view.length {
        let elem = ta_elem(ctx, &view, i);
        let idx_val = value::encode_f64(i as f64);
        let keep = match ctx.call_js(cb, this_arg, &[elem, idx_val, this_val]) {
            Ok(v) => value::is_truthy(v),
            Err(_) => return value::encode_undefined(),
        };
        if keep {
            results.push(elem);
        }
    }
    let new_arr = ctx.alloc_array(results.len() as u32);
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    for (j, elem) in results.iter().enumerate() {
        ctx.array_write_elem(new_arr, j as u32, *elem);
    }
    ctx.array_write_length(new_arr, results.len() as u32);
    new_arr
}

/// TypedArray.prototype.reduce
pub fn typedarray_proto_reduce<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_undefined();
    };
    let cb = ctx.read_call_arg(
        wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
        0,
    );
    if !ctx.is_callable(cb) {
        return value::encode_undefined();
    }
    let has_init = args_count > 1;
    let init = if has_init {
        ctx.read_call_arg(
            wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
            1,
        )
    } else {
        value::encode_undefined()
    };
    if view.length == 0 && !has_init {
        return value::encode_undefined();
    }
    let mut acc = if has_init {
        init
    } else {
        ta_elem(ctx, &view, 0)
    };
    let start = if has_init { 0 } else { 1 };
    for i in start..view.length {
        let elem = ta_elem(ctx, &view, i);
        let idx_val = value::encode_f64(i as f64);
        acc = match ctx.call_js(
            cb,
            value::encode_undefined(),
            &[acc, elem, idx_val, this_val],
        ) {
            Ok(v) => v,
            Err(_) => return value::encode_undefined(),
        };
    }
    acc
}

/// TypedArray.prototype.reduceRight
pub fn typedarray_proto_reduce_right<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_undefined();
    };
    let cb = ctx.read_call_arg(
        wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
        0,
    );
    if !ctx.is_callable(cb) {
        return value::encode_undefined();
    }
    let has_init = args_count > 1;
    let init = if has_init {
        ctx.read_call_arg(
            wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
            1,
        )
    } else {
        value::encode_undefined()
    };
    if view.length == 0 && !has_init {
        return value::encode_undefined();
    }
    let mut acc = if has_init {
        init
    } else {
        ta_elem(ctx, &view, view.length - 1)
    };
    let end = if has_init {
        view.length as i32 - 1
    } else {
        view.length as i32 - 2
    };
    if end < 0 {
        return acc;
    }
    for i in (0..=end as u32).rev() {
        let elem = ta_elem(ctx, &view, i);
        let idx_val = value::encode_f64(i as f64);
        acc = match ctx.call_js(
            cb,
            value::encode_undefined(),
            &[acc, elem, idx_val, this_val],
        ) {
            Ok(v) => v,
            Err(_) => return value::encode_undefined(),
        };
    }
    acc
}

/// TypedArray.prototype.find
pub fn typedarray_proto_find<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_undefined();
    };
    let Some((cb, this_arg)) = read_cb_and_this(ctx, args_base, args_count) else {
        return value::encode_undefined();
    };
    for i in 0..view.length {
        let elem = ta_elem(ctx, &view, i);
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = ctx.call_js(cb, this_arg, &[elem, idx_val, this_val])
            && value::is_truthy(r)
        {
            return elem;
        }
    }
    value::encode_undefined()
}

/// TypedArray.prototype.findIndex
pub fn typedarray_proto_find_index<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_f64(-1.0);
    };
    let Some((cb, this_arg)) = read_cb_and_this(ctx, args_base, args_count) else {
        return value::encode_f64(-1.0);
    };
    for i in 0..view.length {
        let elem = ta_elem(ctx, &view, i);
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = ctx.call_js(cb, this_arg, &[elem, idx_val, this_val])
            && value::is_truthy(r)
        {
            return value::encode_f64(i as f64);
        }
    }
    value::encode_f64(-1.0)
}

/// TypedArray.prototype.some
pub fn typedarray_proto_some<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_bool(false);
    };
    let Some((cb, this_arg)) = read_cb_and_this(ctx, args_base, args_count) else {
        return value::encode_bool(false);
    };
    for i in 0..view.length {
        let elem = ta_elem(ctx, &view, i);
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = ctx.call_js(cb, this_arg, &[elem, idx_val, this_val])
            && value::is_truthy(r)
        {
            return value::encode_bool(true);
        }
    }
    value::encode_bool(false)
}

/// TypedArray.prototype.every
pub fn typedarray_proto_every<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_bool(true);
    };
    let Some((cb, this_arg)) = read_cb_and_this(ctx, args_base, args_count) else {
        return value::encode_bool(true);
    };
    for i in 0..view.length {
        let elem = ta_elem(ctx, &view, i);
        let idx_val = value::encode_f64(i as f64);
        match ctx.call_js(cb, this_arg, &[elem, idx_val, this_val]) {
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
