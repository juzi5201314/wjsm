//! WeakRef / FinalizationRegistry（算法在此；表操作走 ExecContext 原语）。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

pub fn weakref_constructor<E: ExecContext>(ctx: &mut E, args_base: i32, args_count: i32) -> Value {
    if args_count < 1 {
        let msg_val = ctx.store_string("TypeError: WeakRef constructor requires a target argument");
        let error_obj = ctx.create_error_object("TypeError", msg_val, value::encode_undefined());
        return value::encode_exception(value::decode_object_handle(error_obj));
    }
    let target = ctx.read_shadow_arg(args_base, 0);
    if !value::is_js_object(target) {
        let msg_val = ctx.store_string("TypeError: WeakRef: target must be an object");
        let error_obj = ctx.create_error_object("TypeError", msg_val, value::encode_undefined());
        return value::encode_exception(value::decode_object_handle(error_obj));
    }
    let Some(target_handle) = ctx.weak_target_handle(target) else {
        let msg_val = ctx.store_string("TypeError: WeakRef: cannot resolve target handle");
        let error_obj = ctx.create_error_object("TypeError", msg_val, value::encode_undefined());
        return value::encode_exception(value::decode_object_handle(error_obj));
    };
    let handle = ctx.weakref_table_push(target_handle);
    let deref_fn = ctx.create_weakref_method("weakref_deref");
    let obj = ctx.alloc_object(2);
    ctx.define_data_property(obj, "__weakref_handle__", value::encode_f64(handle as f64));
    ctx.define_data_property(obj, "deref", deref_fn);
    obj
}

pub fn weakref_proto_deref<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    if !value::is_object(this_val) {
        return value::encode_undefined();
    }
    let handle_val = ctx.read_property_by_string_key(this_val, "__weakref_handle__");
    if value::is_undefined(handle_val) {
        return value::encode_undefined();
    }
    let handle = value::decode_f64(handle_val) as u32;
    let Some(target_handle) = ctx.weakref_table_get_target(handle) else {
        return value::encode_undefined();
    };
    if !ctx.handle_is_live(target_handle) {
        return value::encode_undefined();
    }
    ctx.encode_handle_as_value(target_handle)
}

pub fn finalization_registry_constructor<E: ExecContext>(
    ctx: &mut E,
    args_base: i32,
    args_count: i32,
) -> Value {
    if args_count < 1 {
        ctx.set_last_error(
            "TypeError: FinalizationRegistry constructor requires a callback argument".to_string(),
        );
        return value::encode_undefined();
    }
    let callback = ctx.read_shadow_arg(args_base, 0);
    if !ctx.is_callable(callback) {
        ctx.set_last_error(
            "TypeError: FinalizationRegistry: callback must be callable".to_string(),
        );
        return value::encode_undefined();
    }
    let obj = ctx.alloc_object(3);
    let object_handle = value::decode_object_handle(obj);
    let handle = ctx.finalization_registry_table_push(object_handle, callback);
    let register_fn = ctx.create_weakref_method("fr_register");
    let unregister_fn = ctx.create_weakref_method("fr_unregister");
    ctx.define_data_property(
        obj,
        "__finalization_registry_handle__",
        value::encode_f64(handle as f64),
    );
    ctx.define_data_property(obj, "register", register_fn);
    ctx.define_data_property(obj, "unregister", unregister_fn);
    obj
}

pub fn finalization_registry_proto_register<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if args_count < 2 {
        return value::encode_undefined();
    }
    let target = ctx.read_shadow_arg(args_base, 0);
    let held_value = ctx.read_shadow_arg(args_base, 1);
    let unregister_token = if args_count >= 3 {
        let token = ctx.read_shadow_arg(args_base, 2);
        if value::is_js_object(token) || value::is_symbol(token) {
            Some(token)
        } else {
            None
        }
    } else {
        None
    };
    if !value::is_js_object(target) {
        return value::encode_undefined();
    }
    let Some(target_handle) = ctx.weak_target_handle(target) else {
        return value::encode_undefined();
    };
    if !value::is_object(this_val) {
        return value::encode_undefined();
    }
    let handle_val = ctx.read_property_by_string_key(this_val, "__finalization_registry_handle__");
    if value::is_undefined(handle_val) {
        return value::encode_undefined();
    }
    let handle = value::decode_f64(handle_val) as u32;
    ctx.finalization_registry_add(handle, target_handle, held_value, unregister_token);
    value::encode_undefined()
}

pub fn finalization_registry_proto_unregister<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    token: Value,
) -> Value {
    if !value::is_object(this_val) {
        return value::encode_bool(false);
    }
    let handle_val = ctx.read_property_by_string_key(this_val, "__finalization_registry_handle__");
    if value::is_undefined(handle_val) {
        return value::encode_bool(false);
    }
    let handle = value::decode_f64(handle_val) as u32;
    value::encode_bool(ctx.finalization_registry_unregister_token(handle, token))
}
