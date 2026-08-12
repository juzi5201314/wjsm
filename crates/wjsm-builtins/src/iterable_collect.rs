//! 可迭代对象抽干（Array.from / Map/Set 构造 / Object.fromEntries 共享）。
//!
//! 控制流在 builtins；再入走 `call_js`，表推进走 ExecContext 原语。

use crate::core_reentrant;
use crate::string_iter::string_iter_advance_unit_pos;
use wjsm_host::{ExecContext, Value};
use wjsm_ir::{value, wk_symbol};

/// 抽干构造器 iterable 参数（Map/Set）：`undefined`/`null` → 空；否则 GetIterator。
pub fn collect_constructor_iterable_values<E: ExecContext>(
    ctx: &mut E,
    source: Value,
) -> Option<Vec<Value>> {
    if value::is_undefined(source) || value::is_null(source) {
        return Some(Vec::new());
    }
    if value::is_object(source) || value::is_array(source) || value::is_function(source) {
        match ctx.get_method_by_name_id(
            source,
            wjsm_host::encode_symbol_name_id(wk_symbol::ITERATOR),
        ) {
            Ok(Some(method)) => {
                let iterator = match ctx.call_js(method, source, &[]) {
                    Ok(v) => v,
                    Err(_) => value::encode_undefined(),
                };
                if value::is_exception(iterator) {
                    return Some(vec![iterator]);
                }
                return collect_iterator_protocol_values(ctx, iterator);
            }
            Ok(None) => {}
            Err(e) => return Some(vec![ctx.make_type_error(&e.to_string())]),
        }
    }
    let iterator = core_reentrant::iterator_from(ctx, source);
    if value::is_exception(iterator) {
        return Some(vec![iterator]);
    }
    collect_iterator_protocol_values(ctx, iterator)
}

/// Array.from 源收集：数组 / 字符串 / TypedArray / 迭代器 / 类数组。
pub fn collect_array_from_values<E: ExecContext>(ctx: &mut E, source: Value) -> Option<Vec<Value>> {
    if value::is_iterator(source) {
        return Some(drain_raw_iterator_via_protocol(ctx, source));
    }
    if value::is_array(source) {
        let len = ctx.array_read_length(source).unwrap_or(0);
        let mut out = Vec::with_capacity(len as usize);
        for i in 0..len {
            out.push(
                ctx.array_elem_at(source, i)
                    .unwrap_or_else(value::encode_undefined),
            );
        }
        return Some(out);
    }
    if value::is_string(source) {
        let string = ctx.get_runtime_string(source);
        let mut out = Vec::new();
        let mut unit_pos = 0usize;
        while unit_pos < string.utf16_len() {
            out.push(string_code_point_at(ctx, &string, unit_pos));
            string_iter_advance_unit_pos(&string, &mut unit_pos);
        }
        return Some(out);
    }
    if let Some(view) = ctx.typedarray_resolve(source) {
        let mut out = Vec::with_capacity(view.length as usize);
        for i in 0..view.length {
            out.push(
                ctx.typedarray_read_elem(&view, i)
                    .unwrap_or_else(value::encode_undefined),
            );
        }
        return Some(out);
    }
    if value::is_object(source) || value::is_function(source) {
        match ctx.get_method_by_name_id(
            source,
            wjsm_host::encode_symbol_name_id(wk_symbol::ITERATOR),
        ) {
            Ok(Some(method)) => {
                let iterator = match ctx.call_js(method, source, &[]) {
                    Ok(v) => v,
                    Err(_) => value::encode_undefined(),
                };
                if value::is_exception(iterator) {
                    return Some(vec![iterator]);
                }
                return collect_iterator_protocol_values(ctx, iterator);
            }
            Ok(None) => {
                // 类数组
                let len_val = ctx.read_property_by_string_key(source, "length");
                let num = ctx.to_number(len_val);
                let f = value::decode_f64(num);
                let len = if !f.is_finite() || f < 0.0 {
                    0u32
                } else {
                    let int = f.trunc().min(9007199254740991.0);
                    if int == 0.0 {
                        0
                    } else {
                        int.rem_euclid(4294967296.0) as u32
                    }
                };
                let mut out = Vec::with_capacity(len as usize);
                for i in 0..len {
                    let key = i.to_string();
                    out.push(ctx.read_property_by_string_key(source, &key));
                }
                return Some(out);
            }
            Err(e) => return Some(vec![ctx.make_type_error(&e.to_string())]),
        }
    }
    None
}

fn collect_iterator_protocol_values<E: ExecContext>(
    ctx: &mut E,
    iterator: Value,
) -> Option<Vec<Value>> {
    // 裸 TAG_ITERATOR 包装成对象迭代器，走 next 协议
    let iterator = if value::is_iterator(iterator) {
        ctx.create_object_iterator(iterator)
    } else {
        iterator
    };
    let next = ctx.read_property_by_string_key(iterator, "next");
    if !ctx.is_callable(next) {
        ctx.set_last_error("TypeError: iterator next is not callable".to_string());
        return None;
    }
    let mut out = Vec::new();
    loop {
        let result = match ctx.call_js(next, iterator, &[]) {
            Ok(v) => v,
            Err(_) => value::encode_undefined(),
        };
        if value::is_exception(result) {
            return Some(vec![result]);
        }
        if !value::is_object(result) && !value::is_function(result) && !value::is_array(result) {
            ctx.set_last_error("TypeError: iterator next must return an object".to_string());
            return None;
        }
        let done = ctx.read_property_by_string_key(result, "done");
        if ctx.to_boolean(done) {
            break;
        }
        out.push(ctx.read_property_by_string_key(result, "value"));
    }
    Some(out)
}

fn drain_raw_iterator_via_protocol<E: ExecContext>(ctx: &mut E, iterator: Value) -> Vec<Value> {
    // 同步推进非 Object 迭代器；ObjectIter 走 next 协议
    let mut out = Vec::new();
    loop {
        let done = core_reentrant::iterator_done(ctx, iterator);
        if value::decode_bool(done) {
            break;
        }
        out.push(ctx.iterator_current_value(iterator));
        let _ = core_reentrant::iterator_next(ctx, iterator);
    }
    out
}

fn string_code_point_at<E: ExecContext>(
    ctx: &mut E,
    string: &wjsm_host::RuntimeString,
    unit_pos: usize,
) -> Value {
    let Some(unit) = string.code_unit_at(unit_pos) else {
        return value::encode_undefined();
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
    ctx.store_runtime_string(string.slice_units(unit_pos..unit_pos + width))
}

/// Map 构造器 entry → (key, value)
pub fn map_entry_pair<E: ExecContext>(ctx: &mut E, entry_val: Value) -> Option<(Value, Value)> {
    if !value::is_js_object(entry_val) && !value::is_array(entry_val) {
        ctx.set_last_error("TypeError: Iterator value is not an entry object".to_string());
        return None;
    }
    if value::is_array(entry_val) {
        let key = ctx
            .array_elem_at(entry_val, 0)
            .unwrap_or_else(value::encode_undefined);
        let val = ctx
            .array_elem_at(entry_val, 1)
            .unwrap_or_else(value::encode_undefined);
        return Some((key, val));
    }
    let key = ctx.read_property_by_string_key(entry_val, "0");
    let val = ctx.read_property_by_string_key(entry_val, "1");
    Some((key, val))
}
