//! 私有字段 get/set/has/accessor bind。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::{constants, value};

/// `env.private_get`
pub fn private_get<E: ExecContext>(ctx: &mut E, obj: Value, key_name_id: i32) -> Value {
    if !value::is_js_object(obj) {
        return ctx.make_type_error("TypeError: Cannot read private member from a non-object");
    }
    let Some(key_name_id) = ctx.canonicalize_name_id(key_name_id as u32) else {
        return value::encode_undefined();
    };
    let handle = value::decode_handle(obj);
    let Some((slot_value, flags, getter, _setter)) = ctx.get_own_property_slot(handle, key_name_id)
    else {
        return ctx.make_type_error(
            "TypeError: Cannot read private member from an object whose class did not declare it",
        );
    };
    // 仅 FLAG_PRIVATE 槽可见
    if flags & constants::FLAG_PRIVATE as u32 == 0 {
        return ctx.make_type_error(
            "TypeError: Cannot read private member from an object whose class did not declare it",
        );
    }
    if flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
        return invoke_private_accessor_get(ctx, getter, obj);
    }
    slot_value
}

/// `env.private_set`
pub fn private_set<E: ExecContext>(ctx: &mut E, obj: Value, key_name_id: i32, val: Value) -> Value {
    if !value::is_js_object(obj) {
        ctx.set_last_error("TypeError: cannot write private member to non-object".to_string());
        return value::encode_undefined();
    }
    let Some(key_name_id) = ctx.canonicalize_name_id(key_name_id as u32) else {
        return value::encode_undefined();
    };
    let handle = value::decode_handle(obj);
    if let Some((_v, flags, _g, setter)) = ctx.get_own_property_slot(handle, key_name_id)
        && flags & constants::FLAG_PRIVATE as u32 != 0
    {
        if flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
            return invoke_private_accessor_set(ctx, setter, obj, val);
        }
        if !ctx.set_property_by_name_id(handle, key_name_id, val) {
            return value::encode_undefined();
        }
        return val;
    }
    if !ctx.define_data_property_with_flags(
        handle,
        key_name_id,
        val,
        constants::FLAG_PRIVATE as u32,
    ) {
        return value::encode_undefined();
    }
    val
}

/// `env.private_accessor_bind`
pub fn private_accessor_bind<E: ExecContext>(
    ctx: &mut E,
    obj: Value,
    key_name_id: i32,
    getter: Value,
    setter: Value,
) -> Value {
    if !value::is_js_object(obj) {
        ctx.set_last_error("TypeError: cannot define private accessor on non-object".to_string());
        return value::encode_undefined();
    }
    let Some(key_name_id) = ctx.canonicalize_name_id(key_name_id as u32) else {
        return value::encode_undefined();
    };
    let handle = value::decode_handle(obj);
    if !ctx.define_accessor_property_with_flags(
        handle,
        key_name_id,
        getter,
        setter,
        constants::FLAG_PRIVATE as u32,
    ) {
        return value::encode_undefined();
    }
    obj
}

/// `env.private_has`
pub fn private_has<E: ExecContext>(ctx: &mut E, obj: Value, key_name_id: i32) -> Value {
    if !value::is_js_object(obj) {
        return value::encode_bool(false);
    }
    let Some(key_name_id) = ctx.canonicalize_name_id(key_name_id as u32) else {
        return value::encode_bool(false);
    };
    let handle = value::decode_handle(obj);
    let found = matches!(
        ctx.get_own_property_slot(handle, key_name_id),
        Some((_, flags, _, _)) if flags & constants::FLAG_PRIVATE as u32 != 0
    );
    value::encode_bool(found)
}

fn invoke_private_accessor_get<E: ExecContext>(ctx: &mut E, getter: Value, obj: Value) -> Value {
    if value::is_undefined(getter) || value::is_null(getter) {
        return ctx.make_type_error("TypeError: Cannot read private member without a getter");
    }
    match ctx.call_js(getter, obj, &[]) {
        Ok(v) => v,
        Err(error) => {
            ctx.set_last_error(format!(
                "private accessor getter callback failed: {error:#}"
            ));
            value::encode_undefined()
        }
    }
}

fn invoke_private_accessor_set<E: ExecContext>(
    ctx: &mut E,
    setter: Value,
    obj: Value,
    val: Value,
) -> Value {
    if value::is_undefined(setter) || value::is_null(setter) {
        ctx.set_last_error("TypeError: Cannot write private member without a setter".to_string());
        return value::encode_undefined();
    }
    match ctx.call_js(setter, obj, &[val]) {
        Ok(v) => v,
        Err(error) => {
            ctx.set_last_error(format!(
                "private accessor setter callback failed: {error:#}"
            ));
            value::encode_undefined()
        }
    }
}
