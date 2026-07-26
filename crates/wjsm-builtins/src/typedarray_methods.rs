//! TypedArray.prototype 同步方法（fill/reverse/indexOf/join 等）。

use crate::object_builtins::same_value_zero;
use wjsm_host::{ExecContext, TypedArrayView, Value};
use wjsm_ir::value;

fn ta_get<E: ExecContext>(ctx: &mut E, view: &TypedArrayView, i: u32) -> Value {
    ctx.typedarray_read_elem(view, i)
        .unwrap_or_else(value::encode_undefined)
}

fn to_rel_index(length: u32, raw: Value, default: i32) -> u32 {
    if value::is_undefined(raw) {
        return default.max(0) as u32;
    }
    let f = value::decode_f64(raw);
    if f.is_nan() {
        return default.max(0) as u32;
    }
    let len = length as i32;
    let idx = if f < 0.0 {
        (len + f as i32).max(0)
    } else {
        (f as i32).min(len)
    };
    idx.max(0) as u32
}

pub fn typedarray_proto_fill<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    value_arg: Value,
    start_raw: Value,
    end_raw: Value,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return this_val;
    };
    let start = to_rel_index(view.length, start_raw, 0);
    let end = to_rel_index(view.length, end_raw, view.length as i32);
    for i in start..end {
        ctx.typedarray_write_elem(&view, i, value_arg);
    }
    this_val
}

pub fn typedarray_proto_reverse<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return this_val;
    };
    for i in 0..view.length / 2 {
        let a = ta_get(ctx, &view, i);
        let b = ta_get(ctx, &view, view.length - 1 - i);
        ctx.typedarray_write_elem(&view, i, b);
        ctx.typedarray_write_elem(&view, view.length - 1 - i, a);
    }
    this_val
}

pub fn typedarray_proto_index_of<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    search: Value,
    from_index: Value,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_f64(-1.0);
    };
    let from_idx = to_rel_index(view.length, from_index, 0);
    for i in from_idx..view.length {
        let elem = ta_get(ctx, &view, i);
        if same_value_zero(ctx, elem, search) {
            return value::encode_f64(i as f64);
        }
    }
    value::encode_f64(-1.0)
}

pub fn typedarray_proto_last_index_of<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    search: Value,
    from_index: Value,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_f64(-1.0);
    };
    if view.length == 0 {
        return value::encode_f64(-1.0);
    }
    let start = if value::is_undefined(from_index) {
        view.length - 1
    } else {
        let f = value::decode_f64(from_index);
        if f.is_nan() {
            return value::encode_f64(-1.0);
        }
        if f < 0.0 {
            let k = view.length as i32 + f as i32;
            if k < 0 {
                return value::encode_f64(-1.0);
            }
            k as u32
        } else {
            (f as u32).min(view.length - 1)
        }
    };
    for i in (0..=start).rev() {
        let elem = ta_get(ctx, &view, i);
        if same_value_zero(ctx, elem, search) {
            return value::encode_f64(i as f64);
        }
    }
    value::encode_f64(-1.0)
}

pub fn typedarray_proto_includes<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    search: Value,
    from_index: Value,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_bool(false);
    };
    let from_idx = to_rel_index(view.length, from_index, 0);
    for i in from_idx..view.length {
        let elem = ta_get(ctx, &view, i);
        if same_value_zero(ctx, elem, search) {
            return value::encode_bool(true);
        }
    }
    value::encode_bool(false)
}

pub fn typedarray_proto_join<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    sep_val: Value,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_undefined();
    };
    let sep = if value::is_undefined(sep_val) {
        ",".to_string()
    } else {
        ctx.value_to_display_string(sep_val)
    };
    let mut parts = Vec::with_capacity(view.length as usize);
    for i in 0..view.length {
        let elem = ta_get(ctx, &view, i);
        if value::is_null(elem) || value::is_undefined(elem) {
            parts.push(String::new());
        } else {
            parts.push(ctx.value_to_display_string(elem));
        }
    }
    ctx.store_string_owned(parts.join(&sep))
}

pub fn typedarray_proto_to_string<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    typedarray_proto_join(ctx, this_val, value::encode_undefined())
}

pub fn typedarray_proto_copy_within<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    target_raw: Value,
    start_raw: Value,
    end_raw: Value,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return this_val;
    };
    let target = to_rel_index(view.length, target_raw, 0) as i32;
    let start = to_rel_index(view.length, start_raw, 0) as i32;
    let end = to_rel_index(view.length, end_raw, view.length as i32) as i32;
    let count = (end - start).max(0).min(view.length as i32 - target).max(0) as u32;
    if count == 0 {
        return this_val;
    }
    if target as u32 > start as u32 {
        for i in (0..count).rev() {
            let elem = ta_get(ctx, &view, start as u32 + i);
            ctx.typedarray_write_elem(&view, target as u32 + i, elem);
        }
    } else {
        for i in 0..count {
            let elem = ta_get(ctx, &view, start as u32 + i);
            ctx.typedarray_write_elem(&view, target as u32 + i, elem);
        }
    }
    this_val
}

pub fn typedarray_proto_at<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    index_raw: Value,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_undefined();
    };
    let len = view.length as i32;
    let idx = if value::is_undefined(index_raw) {
        0
    } else {
        let f = value::decode_f64(index_raw);
        if f.is_nan() {
            0
        } else if f < 0.0 {
            len + f as i32
        } else {
            f as i32
        }
    };
    if idx < 0 || idx >= len {
        return value::encode_undefined();
    }
    ta_get(ctx, &view, idx as u32)
}

/// entries/keys/values 返回数组迭代器（与 host 现有行为对齐：普通 Array iterator）。
pub fn typedarray_proto_entries<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_undefined();
    };
    let arr = ctx.alloc_array(view.length);
    for i in 0..view.length {
        let elem = ta_get(ctx, &view, i);
        let pair = ctx.alloc_array(2);
        ctx.array_write_elem(pair, 0, value::encode_f64(i as f64));
        ctx.array_write_elem(pair, 1, elem);
        ctx.array_write_length(pair, 2);
        ctx.array_write_elem(arr, i, pair);
    }
    ctx.array_write_length(arr, view.length);
    ctx.create_array_iterator(arr)
}

pub fn typedarray_proto_keys<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_undefined();
    };
    let arr = ctx.alloc_array(view.length);
    for i in 0..view.length {
        ctx.array_write_elem(arr, i, value::encode_f64(i as f64));
    }
    ctx.array_write_length(arr, view.length);
    ctx.create_array_iterator(arr)
}

pub fn typedarray_proto_values<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_val) else {
        return value::encode_undefined();
    };
    let arr = ctx.alloc_array(view.length);
    for i in 0..view.length {
        let elem = ta_get(ctx, &view, i);
        ctx.array_write_elem(arr, i, elem);
    }
    ctx.array_write_length(arr, view.length);
    ctx.create_array_iterator(arr)
}

pub fn typedarray_proto_length<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    ctx.typedarray_resolve(this_val)
        .map(|v| value::encode_f64(v.length as f64))
        .unwrap_or_else(|| value::encode_f64(0.0))
}

pub fn typedarray_proto_byte_length<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    ctx.typedarray_resolve(this_val)
        .map(|v| value::encode_f64((v.length as u32 * v.element_size as u32) as f64))
        .unwrap_or_else(|| value::encode_f64(0.0))
}

pub fn typedarray_proto_byte_offset<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    ctx.typedarray_resolve(this_val)
        .map(|v| value::encode_f64(v.byte_offset as f64))
        .unwrap_or_else(|| value::encode_f64(0.0))
}
