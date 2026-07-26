//! Array exotic 方法算法（concat/slice/fill/flat/setLength 等）。
//!
//! 低层堆操作走 ExecContext；本模块持有 ECMAScript 控制流。

use crate::object_builtins::same_value_zero;
use wjsm_host::{ExecContext, Value};
use wjsm_ir::{value, wk_symbol};

const MAX_ARRAY_LENGTH: u32 = u32::MAX;
const ARRAY_LENGTH_RANGE_ERROR: &str = "Invalid array length";

fn array_length_to_uint32(n: f64) -> u32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    let v = n.trunc().rem_euclid(4294967296.0);
    v as u32
}

fn array_get_or_undefined<E: ExecContext>(ctx: &mut E, arr: Value, i: u32) -> Value {
    ctx.array_elem_at(arr, i)
        .unwrap_or_else(value::encode_undefined)
}

/// ECMAScript §23.1.3.2 ArraySetLength
pub fn array_set_length<E: ExecContext>(ctx: &mut E, arr: Value, len_val: Value) -> Value {
    if !value::is_array(arr) || !ctx.resolve_array(arr) {
        return arr;
    }
    let old_len = ctx.array_read_length(arr).unwrap_or(0);

    if same_value_zero(ctx, len_val, value::encode_f64(old_len as f64)) {
        return arr;
    }

    let num = ctx.to_number(len_val);
    let new_len = array_length_to_uint32(value::decode_f64(num));

    if !same_value_zero(ctx, num, value::encode_f64(new_len as f64)) {
        ctx.set_last_error(format!("RangeError: {ARRAY_LENGTH_RANGE_ERROR}"));
        return arr;
    }

    if new_len < old_len {
        for i in new_len..old_len {
            ctx.array_write_hole(arr, i);
        }
    } else if new_len > old_len {
        if !ctx.array_ensure_capacity(arr, new_len) {
            ctx.set_last_error(format!("RangeError: {ARRAY_LENGTH_RANGE_ERROR}"));
            return arr;
        }
        for i in old_len..new_len {
            ctx.array_write_hole(arr, i);
        }
    }
    ctx.array_write_length(arr, new_len);
    arr
}

fn is_concat_spreadable<E: ExecContext>(ctx: &mut E, o: Value) -> Result<bool, Value> {
    if !value::is_js_object(o) {
        return Ok(false);
    }
    let name_id = wjsm_host::encode_symbol_name_id(wk_symbol::IS_CONCAT_SPREADABLE);
    let spreadable = ctx.get_property_by_name_id(o, name_id);
    if value::is_exception(spreadable) {
        return Err(spreadable);
    }
    if !value::is_undefined(spreadable) {
        return Ok(ctx.to_boolean(spreadable));
    }
    Ok(value::is_array(o))
}

fn array_concat_to_length<E: ExecContext>(ctx: &mut E, len_val: Value) -> Result<u32, Value> {
    let num = ctx.to_number(len_val);
    let f = value::decode_f64(num);
    if !f.is_finite() {
        return Ok(0);
    }
    let int = f.trunc();
    if int < 0.0 {
        return Ok(0);
    }
    const MAX_SAFE: f64 = 9007199254740991.0;
    Ok(array_length_to_uint32(int.min(MAX_SAFE)))
}

fn concat_get<E: ExecContext>(ctx: &mut E, obj: Value, prop: Value) -> Value {
    ctx.reflect_get_sync(obj, prop, obj)
}

fn concat_element_contribution<E: ExecContext>(ctx: &mut E, e: Value) -> Result<usize, Value> {
    if !is_concat_spreadable(ctx, e)? {
        return Ok(1);
    }
    if value::is_array(e) {
        return Ok(ctx.array_read_length(e).unwrap_or(0) as usize);
    }
    let len_prop = ctx.store_string("length");
    let len_val = concat_get(ctx, e, len_prop);
    if value::is_exception(len_val) {
        return Err(len_val);
    }
    Ok(array_concat_to_length(ctx, len_val)? as usize)
}

fn concat_append_element<E: ExecContext>(
    ctx: &mut E,
    new_arr: Value,
    write_idx: &mut u32,
    e: Value,
) -> Result<(), Value> {
    if !is_concat_spreadable(ctx, e)? {
        ctx.array_write_elem(new_arr, *write_idx, e);
        *write_idx += 1;
        return Ok(());
    }
    if value::is_array(e) {
        let arg_len = ctx.array_read_length(e).unwrap_or(0);
        for j in 0..arg_len {
            if let Some(elem) = ctx.array_elem_at(e, j) {
                ctx.array_write_elem(new_arr, *write_idx, elem);
                *write_idx += 1;
            }
        }
        return Ok(());
    }
    let len_prop = ctx.store_string("length");
    let len_val = concat_get(ctx, e, len_prop);
    if value::is_exception(len_val) {
        return Err(len_val);
    }
    let len_u32 = array_concat_to_length(ctx, len_val)?;
    for j in 0..len_u32 {
        let elem = concat_get(ctx, e, value::encode_f64(j as f64));
        if value::is_exception(elem) {
            return Err(elem);
        }
        ctx.array_write_elem(new_arr, *write_idx, elem);
        *write_idx += 1;
    }
    Ok(())
}

/// Array.prototype.concat 两参数路径
pub fn array_concat_two<E: ExecContext>(ctx: &mut E, left: Value, right: Value) -> Value {
    if !ctx.resolve_array(left) {
        return value::encode_undefined();
    }
    let left_len = ctx.array_read_length(left).unwrap_or(0);
    let add_right = match concat_element_contribution(ctx, right) {
        Ok(n) => n,
        Err(exc) => return exc,
    };
    let Some(total_len) = (left_len as usize).checked_add(add_right) else {
        return ctx.make_range_error(ARRAY_LENGTH_RANGE_ERROR);
    };
    let Ok(total_len_u32) = u32::try_from(total_len) else {
        return ctx.make_range_error(ARRAY_LENGTH_RANGE_ERROR);
    };
    let new_arr = ctx.array_species_create(left, total_len_u32);
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    let mut write_idx = 0u32;
    for i in 0..left_len {
        if let Some(elem) = ctx.array_elem_at(left, i) {
            ctx.array_write_elem(new_arr, write_idx, elem);
            write_idx += 1;
        }
    }
    if let Err(exc) = concat_append_element(ctx, new_arr, &mut write_idx, right) {
        return exc;
    }
    ctx.array_write_length(new_arr, write_idx);
    new_arr
}

/// Array.prototype.concat 变参
pub fn array_concat_args<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let mut total_len = 0usize;
    let mut items: Vec<Value> = Vec::with_capacity(1 + args_count.max(0) as usize);
    items.push(this_val);
    for i in 0..args_count.max(0) as u32 {
        items.push(ctx.read_shadow_arg(args_base, i));
    }
    for &e in &items {
        let add = match concat_element_contribution(ctx, e) {
            Ok(n) => n,
            Err(exc) => return exc,
        };
        let Some(next_len) = total_len.checked_add(add) else {
            return ctx.make_range_error(ARRAY_LENGTH_RANGE_ERROR);
        };
        total_len = next_len;
    }
    let Ok(total_len_u32) = u32::try_from(total_len) else {
        return ctx.make_range_error(ARRAY_LENGTH_RANGE_ERROR);
    };
    let new_arr = ctx.array_species_create(this_val, total_len_u32);
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    let mut write_idx = 0u32;
    for &e in &items {
        if let Err(exc) = concat_append_element(ctx, new_arr, &mut write_idx, e) {
            return exc;
        }
    }
    ctx.array_write_length(new_arr, write_idx);
    new_arr
}

fn to_integer_index(len: i32, arg: Value, default: i32) -> i32 {
    if value::is_undefined(arg) {
        return default;
    }
    if !value::is_f64(arg) {
        return default;
    }
    let f = value::decode_f64(arg);
    if f.is_nan() {
        return default;
    }
    if f < 0.0 {
        (len + f as i32).max(0)
    } else {
        (f as i32).min(len)
    }
}

/// Array.prototype.slice（支持 array-like：有 length + 索引属性的对象）
pub fn array_slice<E: ExecContext>(
    ctx: &mut E,
    arr: Value,
    start_arg: Value,
    end_arg: Value,
) -> Value {
    let len = array_like_length(ctx, arr) as i32;
    let start = to_integer_index(len, start_arg, 0);
    let end = if value::is_undefined(end_arg) {
        len
    } else {
        to_integer_index(len, end_arg, len)
    };
    let count = (end - start).max(0) as u32;
    let new_arr = if value::is_array(arr) {
        ctx.array_species_create(arr, count)
    } else {
        ctx.alloc_array(count)
    };
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    for i in 0..count {
        let elem = array_like_get(ctx, arr, start as u32 + i);
        ctx.array_write_elem(new_arr, i, elem);
    }
    ctx.array_write_length(new_arr, count);
    new_arr
}

fn array_like_length<E: ExecContext>(ctx: &mut E, this_val: Value) -> u32 {
    if value::is_array(this_val) {
        return ctx.array_read_length(this_val).unwrap_or(0);
    }
    if value::is_object(this_val) || value::is_function(this_val) {
        let len_val = ctx.read_property_by_string_key(this_val, "length");
        let n = value::decode_f64(ctx.to_number(len_val));
        if !n.is_finite() || n <= 0.0 {
            return 0;
        }
        return n.trunc().min(u32::MAX as f64) as u32;
    }
    if value::is_string(this_val) || value::is_runtime_string_handle(this_val) {
        return ctx.string_utf16_len(this_val).unwrap_or(0);
    }
    0
}

fn array_like_get<E: ExecContext>(ctx: &mut E, this_val: Value, index: u32) -> Value {
    if value::is_array(this_val) {
        return array_get_or_undefined(ctx, this_val, index);
    }
    if value::is_object(this_val) || value::is_function(this_val) {
        let key = index.to_string();
        return ctx.read_property_by_string_key(this_val, &key);
    }
    if value::is_string(this_val) || value::is_runtime_string_handle(this_val) {
        let s = ctx.get_runtime_string(this_val);
        if let Some(unit) = s.code_unit_at(index as usize) {
            return ctx.store_runtime_string(wjsm_host::RuntimeString::from_utf16_code_unit(unit));
        }
        return value::encode_undefined();
    }
    value::encode_undefined()
}

/// Array.prototype.fill
pub fn array_fill<E: ExecContext>(
    ctx: &mut E,
    arr: Value,
    val: Value,
    start_arg: Value,
    end_arg: Value,
) -> Value {
    if !ctx.resolve_array(arr) {
        return arr;
    }
    let len = ctx.array_read_length(arr).unwrap_or(0) as i32;
    let start = to_integer_index(len, start_arg, 0);
    let end = if value::is_undefined(end_arg) {
        len
    } else {
        to_integer_index(len, end_arg, len)
    };
    for i in start..end {
        ctx.array_write_elem(arr, i as u32, val);
    }
    arr
}

/// Array.prototype.flat
pub fn array_flat<E: ExecContext>(ctx: &mut E, arr: Value, depth_arg: Value) -> Value {
    let depth = if value::is_undefined(depth_arg) {
        1u32
    } else if value::is_f64(depth_arg) {
        let d = value::decode_f64(depth_arg);
        if d.is_nan() {
            0
        } else {
            let i = d.trunc() as i64;
            if i <= 0 { 0 } else { i as u32 }
        }
    } else {
        1
    };
    let mut elements = Vec::new();
    flatten(ctx, arr, depth, &mut elements);
    let new_arr = ctx.array_species_create(arr, elements.len() as u32);
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    for (i, elem) in elements.iter().enumerate() {
        ctx.array_write_elem(new_arr, i as u32, *elem);
    }
    ctx.array_write_length(new_arr, elements.len() as u32);
    new_arr
}

fn flatten<E: ExecContext>(ctx: &mut E, arr: Value, depth: u32, elements: &mut Vec<Value>) {
    if !ctx.resolve_array(arr) {
        elements.push(arr);
        return;
    }
    let len = ctx.array_read_length(arr).unwrap_or(0);
    for i in 0..len {
        if let Some(elem) = ctx.array_elem_at(arr, i) {
            if depth > 0 && value::is_array(elem) {
                flatten(ctx, elem, depth - 1, elements);
            } else {
                elements.push(elem);
            }
        }
    }
}

/// Array.prototype.push（多参数）
pub fn arr_proto_push<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    let count = args_count.max(0) as u32;
    if len.checked_add(count).is_none() || len.saturating_add(count) > MAX_ARRAY_LENGTH {
        return ctx.make_range_error(ARRAY_LENGTH_RANGE_ERROR);
    }
    for index in 0..count {
        let argument = ctx.read_shadow_arg(args_base, index);
        let _ = ctx.array_push(this_val, argument);
    }
    value::encode_f64((len + count) as f64)
}

/// Array.prototype.pop
pub fn arr_proto_pop<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    if len == 0 {
        return value::encode_undefined();
    }
    let new_len = len - 1;
    let val = array_get_or_undefined(ctx, this_val, new_len);
    ctx.array_write_length(this_val, new_len);
    val
}

/// Function.prototype.bind
pub fn func_bind<E: ExecContext>(
    ctx: &mut E,
    func: Value,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let mut bound_args = Vec::with_capacity(args_count.max(0) as usize);
    for i in 0..args_count.max(0) as u32 {
        bound_args.push(ctx.read_shadow_arg(args_base, i));
    }
    ctx.create_bound_function(func, this_val, bound_args)
}

/// Object.is
pub fn object_is<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    value::encode_bool(crate::object_builtins::same_value(ctx, a, b))
}

/// Array.isArray
pub fn array_is_array<E: ExecContext>(_ctx: &mut E, val: Value) -> Value {
    value::encode_bool(value::is_array(val))
}

/// Array.of
pub fn array_of<E: ExecContext>(ctx: &mut E, args_base: i32, args_count: i32) -> Value {
    let count = args_count.max(0) as u32;
    let arr = ctx.alloc_array(count);
    for i in 0..count {
        let v = ctx.read_shadow_arg(args_base, i);
        ctx.array_write_elem(arr, i, v);
    }
    ctx.array_write_length(arr, count);
    arr
}

/// Array.prototype.shift
pub fn arr_proto_shift<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    if len == 0 {
        return value::encode_undefined();
    }
    let val = array_get_or_undefined(ctx, this_val, 0);
    for i in 1..len {
        let elem = array_get_or_undefined(ctx, this_val, i);
        ctx.array_write_elem(this_val, i - 1, elem);
    }
    ctx.array_write_length(this_val, len - 1);
    val
}

/// Array.prototype.unshift
pub fn arr_proto_unshift<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    let add = args_count.max(0) as u32;
    if len.checked_add(add).is_none() {
        return ctx.make_range_error(ARRAY_LENGTH_RANGE_ERROR);
    }
    let new_len = len + add;
    if !ctx.array_ensure_capacity(this_val, new_len) {
        return ctx.make_range_error(ARRAY_LENGTH_RANGE_ERROR);
    }
    for i in (0..len).rev() {
        let elem = array_get_or_undefined(ctx, this_val, i);
        ctx.array_write_elem(this_val, i + add, elem);
    }
    for i in 0..add {
        let arg = ctx.read_shadow_arg(args_base, i);
        ctx.array_write_elem(this_val, i, arg);
    }
    ctx.array_write_length(this_val, new_len);
    value::encode_f64(new_len as f64)
}

/// Array.prototype.at
pub fn arr_proto_at<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0) as i32;
    let idx = if args_count > 0 {
        let i_f64 = value::decode_f64(ctx.read_shadow_arg(args_base, 0));
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
    array_get_or_undefined(ctx, this_val, idx as u32)
}

/// Array.prototype.toReversed
pub fn arr_proto_to_reversed<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0);
    let new_arr = ctx.array_species_create(this_val, len);
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    for i in 0..len {
        let elem = array_get_or_undefined(ctx, this_val, len - 1 - i);
        ctx.array_write_elem(new_arr, i, elem);
    }
    ctx.array_write_length(new_arr, len);
    new_arr
}

/// Array.prototype.with
pub fn arr_proto_with<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0) as i32;
    let idx = if args_count > 0 {
        let i_f64 = value::decode_f64(ctx.read_shadow_arg(args_base, 0));
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
        return ctx.make_range_error("Invalid index");
    }
    let value_arg = if args_count > 1 {
        ctx.read_shadow_arg(args_base, 1)
    } else {
        value::encode_undefined()
    };
    let new_arr = ctx.array_species_create(this_val, len as u32);
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    for i in 0..len as u32 {
        if i == idx as u32 {
            ctx.array_write_elem(new_arr, i, value_arg);
        } else {
            let elem = array_get_or_undefined(ctx, this_val, i);
            ctx.array_write_elem(new_arr, i, elem);
        }
    }
    ctx.array_write_length(new_arr, len as u32);
    new_arr
}

/// Array.prototype.copyWithin
pub fn arr_proto_copy_within<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if !ctx.resolve_array(this_val) {
        return this_val;
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0) as i32;
    let target = if args_count > 0 {
        to_integer_index(len, ctx.read_shadow_arg(args_base, 0), 0)
    } else {
        0
    };
    let start = if args_count > 1 {
        to_integer_index(len, ctx.read_shadow_arg(args_base, 1), 0)
    } else {
        0
    };
    let end = if args_count > 2 {
        to_integer_index(len, ctx.read_shadow_arg(args_base, 2), len)
    } else {
        len
    };
    let count = (end - start).max(0).min(len - target).max(0) as u32;
    if count == 0 {
        return this_val;
    }
    // direction: if target > start, copy backwards
    if target as u32 > start as u32 {
        for i in (0..count).rev() {
            if let Some(elem) = ctx.array_elem_at(this_val, start as u32 + i) {
                ctx.array_write_elem(this_val, target as u32 + i, elem);
            } else {
                ctx.array_write_hole(this_val, target as u32 + i);
            }
        }
    } else {
        for i in 0..count {
            if let Some(elem) = ctx.array_elem_at(this_val, start as u32 + i) {
                ctx.array_write_elem(this_val, target as u32 + i, elem);
            } else {
                ctx.array_write_hole(this_val, target as u32 + i);
            }
        }
    }
    this_val
}

/// Array.prototype.splice（删除 + 插入）
pub fn arr_proto_splice<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0) as i32;
    let start = if args_count > 0 {
        to_integer_index(len, ctx.read_shadow_arg(args_base, 0), 0)
    } else {
        0
    };
    let delete_count = if args_count > 1 {
        let d = value::decode_f64(ctx.read_shadow_arg(args_base, 1));
        if d.is_nan() || d < 0.0 {
            0
        } else {
            (d.trunc() as i32).min(len - start).max(0)
        }
    } else if args_count == 1 {
        (len - start).max(0)
    } else {
        0
    };
    let insert_count = (args_count - 2).max(0) as u32;
    let deleted = ctx.array_species_create(this_val, delete_count as u32);
    if ctx.resolve_array(deleted) {
        for i in 0..delete_count as u32 {
            let elem = array_get_or_undefined(ctx, this_val, start as u32 + i);
            ctx.array_write_elem(deleted, i, elem);
        }
        ctx.array_write_length(deleted, delete_count as u32);
    }
    let new_len = (len as u32) - (delete_count as u32) + insert_count;
    if !ctx.array_ensure_capacity(this_val, new_len) {
        return ctx.make_range_error(ARRAY_LENGTH_RANGE_ERROR);
    }
    // shift tail
    let tail_start = start as u32 + delete_count as u32;
    let tail_len = (len as u32).saturating_sub(tail_start);
    let new_tail_start = start as u32 + insert_count;
    if new_tail_start > tail_start {
        for i in (0..tail_len).rev() {
            if let Some(elem) = ctx.array_elem_at(this_val, tail_start + i) {
                ctx.array_write_elem(this_val, new_tail_start + i, elem);
            } else {
                ctx.array_write_hole(this_val, new_tail_start + i);
            }
        }
    } else if new_tail_start < tail_start {
        for i in 0..tail_len {
            if let Some(elem) = ctx.array_elem_at(this_val, tail_start + i) {
                ctx.array_write_elem(this_val, new_tail_start + i, elem);
            } else {
                ctx.array_write_hole(this_val, new_tail_start + i);
            }
        }
    }
    for i in 0..insert_count {
        let arg = ctx.read_shadow_arg(args_base, 2 + i);
        ctx.array_write_elem(this_val, start as u32 + i, arg);
    }
    ctx.array_write_length(this_val, new_len);
    deleted
}

/// Array.prototype.toSpliced
pub fn arr_proto_to_spliced<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if !ctx.resolve_array(this_val) {
        return value::encode_undefined();
    }
    let len = ctx.array_read_length(this_val).unwrap_or(0) as i32;
    let start = if args_count > 0 {
        to_integer_index(len, ctx.read_shadow_arg(args_base, 0), 0)
    } else {
        0
    };
    let delete_count = if args_count > 1 {
        let d = value::decode_f64(ctx.read_shadow_arg(args_base, 1));
        if d.is_nan() || d < 0.0 {
            0
        } else {
            (d.trunc() as i32).min(len - start).max(0)
        }
    } else if args_count == 1 {
        (len - start).max(0)
    } else {
        0
    };
    let insert_count = (args_count - 2).max(0) as u32;
    let new_len = (len as u32) - (delete_count as u32) + insert_count;
    let new_arr = ctx.array_species_create(this_val, new_len);
    if !ctx.resolve_array(new_arr) {
        return value::encode_undefined();
    }
    let mut w = 0u32;
    for i in 0..start as u32 {
        let elem = array_get_or_undefined(ctx, this_val, i);
        ctx.array_write_elem(new_arr, w, elem);
        w += 1;
    }
    for i in 0..insert_count {
        let arg = ctx.read_shadow_arg(args_base, 2 + i);
        ctx.array_write_elem(new_arr, w, arg);
        w += 1;
    }
    for i in (start + delete_count) as u32..len as u32 {
        let elem = array_get_or_undefined(ctx, this_val, i);
        ctx.array_write_elem(new_arr, w, elem);
        w += 1;
    }
    ctx.array_write_length(new_arr, new_len);
    new_arr
}

/// Array.prototype.lastIndexOf（shadow args 路径）
pub fn arr_proto_last_index_of<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if !ctx.resolve_array(this_val) {
        return value::encode_f64(-1.0);
    }
    let search = ctx.read_shadow_arg(args_base, 0);
    let from = if args_count > 1 {
        ctx.read_shadow_arg(args_base, 1)
    } else {
        value::encode_undefined()
    };
    crate::timers_arrays::arr_last_index_of(ctx, this_val, search, from)
}

/// Array.prototype.includes（shadow args）
pub fn arr_proto_includes<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let search = ctx.read_shadow_arg(args_base, 0);
    let from = if args_count > 1 {
        ctx.read_shadow_arg(args_base, 1)
    } else {
        value::encode_undefined()
    };
    crate::timers_arrays::arr_includes_from(ctx, this_val, search, from)
}

/// Array.prototype.indexOf（shadow args）
pub fn arr_proto_index_of_args<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let search = ctx.read_shadow_arg(args_base, 0);
    let from = if args_count > 1 {
        ctx.read_shadow_arg(args_base, 1)
    } else {
        value::encode_undefined()
    };
    crate::timers_arrays::arr_index_of(ctx, this_val, search, from)
}

/// Array.prototype.join（shadow args）
pub fn arr_proto_join_args<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let sep = if args_count > 0 {
        ctx.read_shadow_arg(args_base, 0)
    } else {
        value::encode_undefined()
    };
    crate::timers_arrays::arr_join(ctx, this_val, sep)
}

/// Array.prototype.concat（shadow args）
pub fn arr_proto_concat_args<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    array_concat_args(ctx, this_val, args_base, args_count)
}

/// Array.prototype.slice（shadow args）
pub fn arr_proto_slice_args<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let start = if args_count > 0 {
        ctx.read_shadow_arg(args_base, 0)
    } else {
        value::encode_undefined()
    };
    let end = if args_count > 1 {
        ctx.read_shadow_arg(args_base, 1)
    } else {
        value::encode_undefined()
    };
    array_slice(ctx, this_val, start, end)
}

/// Array.prototype.fill（shadow args）
pub fn arr_proto_fill_args<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let val = ctx.read_shadow_arg(args_base, 0);
    let start = if args_count > 1 {
        ctx.read_shadow_arg(args_base, 1)
    } else {
        value::encode_undefined()
    };
    let end = if args_count > 2 {
        ctx.read_shadow_arg(args_base, 2)
    } else {
        value::encode_undefined()
    };
    array_fill(ctx, this_val, val, start, end)
}

/// Array.prototype.flat（shadow args）
pub fn arr_proto_flat_args<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let depth = if args_count > 0 {
        ctx.read_shadow_arg(args_base, 0)
    } else {
        value::encode_undefined()
    };
    array_flat(ctx, this_val, depth)
}

/// Array.prototype.reverse
pub fn arr_proto_reverse<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    crate::timers_arrays::arr_reverse(ctx, this_val)
}

/// `Array.from(items, mapFn?)`：支持可迭代对象（@@iterator）、类数组、字符串、TypedArray。
pub async fn array_from_impl<E: ExecContext>(
    ctx: &mut E,
    source: Value,
    map_fn: Value,
) -> Value {
    let has_map_fn = ctx.is_callable(map_fn);
    let Some(values) = crate::iterable_collect::collect_array_from_values(ctx, source).await
    else {
        ctx.set_last_error(
            "TypeError: Array.from requires an array-like or iterable object".to_string(),
        );
        return value::encode_undefined();
    };

    let count = values.len() as u32;
    let result = ctx.alloc_array(count);
    for (i, raw) in values.into_iter().enumerate() {
        let mapped = if has_map_fn {
            let idx_val = value::encode_f64(i as f64);
            match ctx.call_js_async(map_fn, value::encode_undefined(), &[raw, idx_val]).await {
                Ok(v) => {
                    if value::is_exception(v) {
                        return v;
                    }
                    v
                }
                Err(_) => value::encode_undefined(),
            }
        } else {
            raw
        };
        ctx.array_write_elem(result, i as u32, mapped);
    }
    ctx.array_write_length(result, count);
    result
}
