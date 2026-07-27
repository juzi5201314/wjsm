//! GetMethod / Get(O, P) — 后端无关实现。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

/// GetMethod 的 symbol name_id 版本。
///
/// `Ok(None)` = undefined/null；`Err(exception)` = 不可调用。
pub fn get_method_by_name_id<E: ExecContext>(
    ctx: &mut E,
    obj: Value,
    name_id: u32,
) -> Result<Option<Value>, Value> {
    let func = get_v_by_name_id(ctx, obj, name_id);
    if value::is_undefined(func) || value::is_null(func) {
        return Ok(None);
    }
    if !ctx.is_callable(func) {
        let msg_val = ctx.store_string("method is not callable");
        let error_obj = ctx.create_error_object("TypeError", msg_val, value::encode_undefined());
        return Err(ctx.push_exception("TypeError", "method is not callable", error_obj));
    }
    Ok(Some(func))
}

/// ECMAScript `Get(O, P)`（支持 string 和 symbol name_id）。
pub fn get_by_name_id<E: ExecContext>(ctx: &mut E, obj: Value, name_id: u32) -> Value {
    if value::is_array(obj) {
        let handle = value::decode_handle(obj);
        if ctx.resolve_handle(handle) {
            if ctx.name_id_matches(name_id, "length") {
                return ctx
                    .array_length(handle)
                    .map(|len| value::encode_f64(len as f64))
                    .unwrap_or_else(value::encode_undefined);
            }
            if let Some(v) = ctx.array_named_prop_get(obj, name_id) {
                return v;
            }
            return match ctx.get_property_slot_on_proto(handle, name_id) {
                Some((_, true, getter)) => invoke_getter(ctx, getter, obj),
                Some((val, false, _)) => val,
                None => value::encode_undefined(),
            };
        }
        // legacy array path
        if ctx.name_id_matches(name_id, "length")
            && let Some(len) = ctx.array_read_length(obj)
        {
            return value::encode_f64(len as f64);
        }
        if let Some(own) = ctx.array_named_prop_get(obj, name_id)
            && !value::is_undefined(own)
        {
            return own;
        }
    }

    if value::is_regexp(obj) {
        return ctx.primitive_regexp_get_property(obj, name_id);
    }
    if value::is_proxy(obj) {
        let Some(prop) = ctx.name_id_to_property_key_value(name_id) else {
            return value::encode_undefined();
        };
        return ctx.reflect_get_sync(obj, prop, obj);
    }
    if value::is_string(obj) {
        if ctx.name_id_matches(name_id, "length") {
            return ctx
                .string_utf16_len(obj)
                .map(|len| value::encode_f64(len as f64))
                .unwrap_or_else(value::encode_undefined);
        }
        return value::encode_undefined();
    }
    if value::is_native_callable(obj) {
        if let Some(n) = ctx.native_eval_function_param_count(obj) {
            if ctx.name_id_matches(name_id, "length") {
                return value::encode_f64(n as f64);
            }
            if ctx.name_id_matches(name_id, "name") {
                return ctx.store_string("");
            }
        }
        if ctx.name_id_matches(name_id, "bigint") && ctx.is_process_hrtime_callable(obj) {
            return ctx.create_process_hrtime_bigint();
        }
        return value::encode_undefined();
    }

    if !value::is_js_object(obj) && !value::is_array(obj) {
        return value::encode_undefined();
    }

    let handle = if value::is_function(obj) || value::is_closure(obj) || value::is_bound(obj) {
        ctx.handle_index_of(obj)
            .unwrap_or_else(|| value::decode_handle(obj))
    } else {
        value::decode_handle(obj)
    };
    if ctx.resolve_handle(handle) {
        return match ctx.get_property_slot_on_proto(handle, name_id) {
            Some((_, true, getter)) => invoke_getter(ctx, getter, obj),
            Some((val, false, _)) => val,
            None => value::encode_undefined(),
        };
    }

    let Some(ptr) = ctx.resolve_object_ptr(obj) else {
        return value::encode_undefined();
    };
    ctx.get_by_name_id_on_proto_chain(obj, ptr, name_id)
        .unwrap_or_else(value::encode_undefined)
}

/// GetV 的 name_id 版本（GetMethod 用；访问器槽返回数据值，不强制 invoke——
/// 与原 get_v 一致：V2 路径直接取 slot.value）。
fn get_v_by_name_id<E: ExecContext>(ctx: &mut E, value_val: Value, name_id: u32) -> Value {
    if value::is_proxy(value_val) {
        let Some(prop) = ctx.name_id_to_property_key_value(name_id) else {
            return value::encode_undefined();
        };
        return ctx.reflect_get_sync(value_val, prop, value_val);
    }
    if value::is_regexp(value_val) {
        return ctx.primitive_regexp_get_property(value_val, name_id);
    }

    let handle = if value::is_function(value_val)
        || value::is_closure(value_val)
        || value::is_bound(value_val)
    {
        ctx.handle_index_of(value_val)
            .unwrap_or_else(|| value::decode_handle(value_val))
    } else {
        value::decode_handle(value_val)
    };
    if (value::is_js_object(value_val) || value::is_array(value_val)) && ctx.resolve_handle(handle)
    {
        if value::is_array(value_val)
            && let Some(v) = ctx.array_named_prop_get(value_val, name_id)
        {
            return v;
        }
        return match ctx.get_property_slot_on_proto(handle, name_id) {
            Some((val, _, _)) => val,
            None => value::encode_undefined(),
        };
    }
    let Some(ptr) = ctx.resolve_object_ptr(value_val) else {
        return value::encode_undefined();
    };
    ctx.read_property_by_name_id_proto_walk(ptr, name_id)
        .unwrap_or_else(value::encode_undefined)
}

/// 沿原型链查找 name_id 属性（数据槽，不调用 getter）。
pub fn read_object_property_by_name_id_proto_walk<E: ExecContext>(
    ctx: &mut E,
    obj_ptr: usize,
    name_id: u32,
) -> Option<Value> {
    ctx.read_property_by_name_id_proto_walk(obj_ptr, name_id)
}

/// 同步调用 getter（后端无关算法；经 `call_native_callable` / `call_js` 落到后端）。
///
/// 可调用性必须走 `ctx.is_callable`（含 Proxy `apply` trap 链），
/// 禁止仅用 `value::is_callable` 的标签判断，以免与 GetMethod 路径不一致。
pub fn invoke_getter<E: ExecContext>(ctx: &mut E, getter: Value, receiver: Value) -> Value {
    if value::is_undefined(getter) || value::is_null(getter) {
        return value::encode_undefined();
    }
    if !ctx.is_callable(getter) {
        return value::encode_undefined();
    }
    if value::is_native_callable(getter) {
        return ctx.call_native_callable(getter, receiver, &[]);
    }
    ctx.call_js(getter, receiver, &[])
        .unwrap_or_else(|_| value::encode_undefined())
}
