//! `instanceof` 与 OrdinaryHasInstance。

use wjsm_host::{ExecContext, Value, encode_symbol_name_id};
use wjsm_ir::{value, wk_symbol};

fn ordinary_has_instance<E: ExecContext>(ctx: &mut E, object: Value, constructor: Value) -> Value {
    if !ctx.is_callable(constructor) {
        return ctx.make_type_error("Right-hand side of instanceof is not callable");
    }
    if !value::is_js_object(object) && !value::is_regexp(object) {
        return value::encode_bool(false);
    }

    let prototype_key = ctx.store_string("prototype");
    let prototype = crate::proxy_reflect_reentrant::reflect_get_impl_with_receiver(
        ctx,
        constructor,
        prototype_key,
        constructor,
    );
    if value::is_exception(prototype) {
        return prototype;
    }
    if !value::is_js_object(prototype) {
        return ctx.make_type_error("Function has non-object prototype property");
    }
    let Some(target_prototype) = ctx.handle_index_of(prototype) else {
        return value::encode_bool(false);
    };

    let mut current = if value::is_regexp(object) {
        let regexp_prototype = ctx.regexp_prototype();
        let Some(handle) = ctx.handle_index_of(regexp_prototype) else {
            return value::encode_bool(false);
        };
        handle
    } else {
        let Some(handle) = ctx.object_proto_handle(object) else {
            return value::encode_bool(false);
        };
        handle
    };

    loop {
        if current == u32::MAX {
            return value::encode_bool(false);
        }
        if current == target_prototype {
            return value::encode_bool(true);
        }
        let Some(next) = ctx.prototype_of(current) else {
            return value::encode_bool(false);
        };
        current = next;
    }
}

/// ECMAScript InstanceofOperator：先查 @@hasInstance，再走 OrdinaryHasInstance。
pub fn op_instanceof<E: ExecContext>(ctx: &mut E, object: Value, constructor: Value) -> Value {
    if !value::is_js_object(constructor) {
        return ctx.make_type_error("Right-hand side of instanceof is not an object");
    }
    let name_id = encode_symbol_name_id(wk_symbol::HAS_INSTANCE);
    match crate::get_method::get_method_by_name_id(ctx, constructor, name_id) {
        Ok(Some(method)) => match ctx.call_js(method, constructor, &[object]) {
            Ok(result) if value::is_exception(result) => result,
            Ok(result) => value::encode_bool(ctx.to_boolean(result)),
            Err(_) => value::encode_undefined(),
        },
        Ok(None) => ordinary_has_instance(ctx, object, constructor),
        Err(exception) => exception,
    }
}
