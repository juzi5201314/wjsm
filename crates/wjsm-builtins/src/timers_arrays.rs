//! Closure 侧表 + 基础数组方法 host imports。
//!
//! join / includes / indexOf 算法在此；concat/slice/fill/flat 等 exotic 仍走
//! ExecContext 数组原语（Phase 3 array_object 完整迁移前）。

use crate::object_builtins::same_value_zero;
use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

pub fn closure_create<E: ExecContext>(ctx: &mut E, func_ref: Value, env_obj: Value) -> Value {
    let func_idx = if value::is_function(func_ref) {
        value::decode_function_idx(func_ref)
    } else if value::is_closure(func_ref) {
        let idx = value::decode_closure_idx(func_ref);
        ctx.closure_func_idx(idx).unwrap_or(0)
    } else {
        0
    };
    ctx.create_closure(func_idx, env_obj)
}

pub fn closure_get_func<E: ExecContext>(ctx: &mut E, closure_idx: i32) -> i32 {
    ctx.closure_func_idx(closure_idx as u32)
        .map(|f| f as i32)
        .unwrap_or(-1)
}

pub fn closure_get_env<E: ExecContext>(ctx: &mut E, closure_idx: i32) -> Value {
    ctx.closure_env(closure_idx as u32)
        .unwrap_or_else(value::encode_undefined)
}

pub fn arr_push<E: ExecContext>(ctx: &mut E, arr: Value, val: Value) -> Value {
    ctx.array_push(arr, val)
}

pub fn arr_push_hole<E: ExecContext>(ctx: &mut E, arr: Value) -> Value {
    ctx.array_push_hole(arr)
}

pub fn arr_pop<E: ExecContext>(ctx: &mut E, arr: Value) -> Value {
    let Some(len) = ctx.array_read_length(arr) else {
        return value::encode_undefined();
    };
    if len == 0 {
        return value::encode_undefined();
    }
    let new_len = len - 1;
    let val = ctx
        .array_elem_at(arr, new_len)
        .unwrap_or_else(value::encode_undefined);
    ctx.array_write_length(arr, new_len);
    val
}

fn array_relative_start(len: u32, from_index: Value) -> u32 {
    if value::is_undefined(from_index) {
        return 0;
    }
    if !value::is_f64(from_index) {
        return 0;
    }
    let f = value::decode_f64(from_index);
    if f.is_nan() {
        return 0;
    }
    if f == f64::INFINITY {
        return len;
    }
    if f == f64::NEG_INFINITY {
        return 0;
    }
    let len_i = len as i64;
    if f < 0.0 {
        let k = f as i64;
        return (len_i + k).max(0).min(len_i) as u32;
    }
    let k = f as i64;
    k.max(0).min(len_i) as u32
}

pub fn arr_includes<E: ExecContext>(ctx: &mut E, arr: Value, val: Value) -> Value {
    arr_includes_from(ctx, arr, val, value::encode_undefined())
}

pub fn arr_includes_from<E: ExecContext>(
    ctx: &mut E,
    arr: Value,
    val: Value,
    from: Value,
) -> Value {
    let Some(len) = ctx.array_read_length(arr) else {
        return value::encode_bool(false);
    };
    let start = array_relative_start(len, from);
    for i in start..len {
        if let Some(elem) = ctx.array_elem_at(arr, i)
            && same_value_zero(ctx, elem, val)
        {
            return value::encode_bool(true);
        }
    }
    value::encode_bool(false)
}

pub fn arr_index_of<E: ExecContext>(ctx: &mut E, arr: Value, val: Value, from_val: Value) -> Value {
    let Some(len) = ctx.array_read_length(arr) else {
        return value::encode_f64(-1.0);
    };
    let start = array_relative_start(len, from_val);
    for i in start..len {
        if let Some(elem) = ctx.array_elem_at(arr, i)
            && same_value_zero(ctx, elem, val)
        {
            return value::encode_f64(i as f64);
        }
    }
    value::encode_f64(-1.0)
}

pub fn arr_last_index_of<E: ExecContext>(
    ctx: &mut E,
    arr: Value,
    val: Value,
    from_val: Value,
) -> Value {
    let Some(len) = ctx.array_read_length(arr) else {
        return value::encode_f64(-1.0);
    };
    if len == 0 {
        return value::encode_f64(-1.0);
    }
    let start = if value::is_undefined(from_val) {
        len - 1
    } else if value::is_f64(from_val) {
        let f = value::decode_f64(from_val);
        if f.is_nan() {
            return value::encode_f64(-1.0);
        }
        if f == f64::INFINITY {
            len - 1
        } else if f == f64::NEG_INFINITY {
            return value::encode_f64(-1.0);
        } else if f < 0.0 {
            let k = (len as i64) + (f as i64);
            if k < 0 {
                return value::encode_f64(-1.0);
            }
            (k as u32).min(len - 1)
        } else {
            (f as i64).max(0).min((len - 1) as i64) as u32
        }
    } else {
        len - 1
    };
    for i in (0..=start).rev() {
        if let Some(elem) = ctx.array_elem_at(arr, i)
            && same_value_zero(ctx, elem, val)
        {
            return value::encode_f64(i as f64);
        }
    }
    value::encode_f64(-1.0)
}

pub fn arr_join<E: ExecContext>(ctx: &mut E, arr: Value, sep_val: Value) -> Value {
    let Some(len) = ctx.array_read_length(arr) else {
        return value::encode_undefined();
    };
    let sep_str = if value::is_undefined(sep_val) {
        ",".to_string()
    } else {
        ctx.value_to_display_string(sep_val)
    };
    let mut parts = Vec::with_capacity(len as usize);
    for i in 0..len {
        match ctx.array_elem_at(arr, i) {
            Some(elem) if value::is_null(elem) || value::is_undefined(elem) => {
                parts.push(String::new());
            }
            Some(elem) => parts.push(ctx.value_to_display_string(elem)),
            None => parts.push(String::new()),
        }
    }
    ctx.store_string_owned(parts.join(&sep_str))
}

pub fn arr_concat<E: ExecContext>(ctx: &mut E, arr1: Value, arr2: Value) -> Value {
    crate::array_object::array_concat_two(ctx, arr1, arr2)
}

pub fn arr_slice<E: ExecContext>(ctx: &mut E, arr: Value, start: Value, end: Value) -> Value {
    crate::array_object::array_slice(ctx, arr, start, end)
}

pub fn arr_fill<E: ExecContext>(
    ctx: &mut E,
    arr: Value,
    val: Value,
    start: Value,
    end: Value,
) -> Value {
    crate::array_object::array_fill(ctx, arr, val, start, end)
}

pub fn arr_reverse<E: ExecContext>(ctx: &mut E, arr: Value) -> Value {
    let Some(len) = ctx.array_read_length(arr) else {
        return arr;
    };
    for i in 0..len / 2 {
        let a = ctx
            .array_elem_at(arr, i)
            .unwrap_or_else(value::encode_undefined);
        let b = ctx
            .array_elem_at(arr, len - 1 - i)
            .unwrap_or_else(value::encode_undefined);
        ctx.array_write_elem(arr, i, b);
        ctx.array_write_elem(arr, len - 1 - i, a);
    }
    arr
}

pub fn arr_flat<E: ExecContext>(ctx: &mut E, arr: Value, depth: Value) -> Value {
    crate::array_object::array_flat(ctx, arr, depth)
}

pub fn arr_init_length<E: ExecContext>(ctx: &mut E, arr: Value, len_val: Value) -> Value {
    crate::array_object::array_set_length(ctx, arr, len_val)
}

pub fn array_set_length<E: ExecContext>(ctx: &mut E, arr: Value, len_val: Value) -> Value {
    crate::array_object::array_set_length(ctx, arr, len_val)
}

pub fn arr_get_length<E: ExecContext>(ctx: &mut E, arr: Value) -> Value {
    ctx.array_read_length(arr)
        .map(|len| value::encode_f64(len as f64))
        .unwrap_or_else(value::encode_undefined)
}
