use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

pub fn create_proxy<E: ExecContext>(ctx: &mut E, target: Value, handler: Value) -> Value {
    if !value::is_js_object(target) {
        return ctx.make_type_error("TypeError: Proxy target must be an object");
    }
    if !value::is_js_object(handler) {
        return ctx.make_type_error("TypeError: Proxy handler must be an object");
    }
    ctx.alloc_proxy(target, handler)
}

pub fn create_revocable_proxy<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    handler: Value,
) -> Value {
    let proxy = create_proxy(ctx, target, handler);
    if value::is_exception(proxy) {
        return proxy;
    }
    let revoke = ctx.create_proxy_revoker(proxy);
    let result = ctx.alloc_object(2);
    ctx.define_data_property(result, "proxy", proxy);
    ctx.define_data_property(result, "revoke", revoke);
    result
}


pub async fn reflect_delete_property<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    property: Value,
) -> Value {
    if !value::is_proxy(target) {
        return crate::proxy_reflect::reflect_delete_property_impl(ctx, target, property);
    }
    let (target, handler) = match crate::proxy_traps::proxy_trap_proxy_entry(
        ctx,
        target,
        "deleteProperty",
    ) {
        Ok(pair) => pair,
        Err(exception) => return exception,
    };
    if let Some(trap) = crate::proxy_traps::proxy_trap_handler_trap(
        ctx,
        handler,
        "deleteProperty",
    ) {
        return match ctx.call_js_async(trap, handler, &[target, property]).await {
            Ok(result) => value::encode_bool(value::is_truthy(result)),
            Err(_) => value::encode_bool(false),
        };
    }
    crate::proxy_reflect::reflect_delete_property_impl(ctx, target, property)
}

pub async fn reflect_apply<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    this_arg: Value,
    args_array: Value,
) -> Value {
    if !ctx.is_callable(target) {
        return ctx.make_type_error("TypeError: Reflect.apply target must be callable");
    }
    let args = match crate::proxy_reflect_async::extract_array_like_elements(ctx, args_array).await {
        Ok(args) => args,
        Err(error) => {
            ctx.set_last_error(error);
            return value::encode_undefined();
        }
    };
    crate::proxy_reflect_async::reflect_apply_impl_async(ctx, target, this_arg, &args).await
}

pub async fn reflect_construct<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    args_array: Value,
    new_target: Value,
) -> Value {
    let new_target = if value::is_undefined(new_target) {
        target
    } else {
        new_target
    };
    if !ctx.is_callable(target) || !ctx.is_callable(new_target) {
        return ctx.make_type_error(
            "TypeError: Reflect.construct target and newTarget must be constructors",
        );
    }
    let args = match crate::proxy_reflect_async::extract_array_like_elements(ctx, args_array).await {
        Ok(args) => args,
        Err(error) => {
            ctx.set_last_error(error);
            return value::encode_undefined();
        }
    };
    crate::proxy_reflect_async::reflect_construct_impl_async(ctx, target, &args, new_target).await
}

pub async fn reflect_get_prototype_of<E: ExecContext>(ctx: &mut E, target: Value) -> Value {
    if !is_object_like(target) && !value::is_regexp(target) {
        ctx.set_last_error("TypeError: Reflect.getPrototypeOf called on non-object".to_string());
        return value::encode_undefined();
    }
    crate::proxy_reflect_async::reflect_get_prototype_of_async(ctx, target).await
}

pub async fn reflect_set_prototype_of<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    prototype: Value,
) -> Value {
    if !is_object_like(target) {
        ctx.set_last_error("TypeError: Reflect.setPrototypeOf called on non-object".to_string());
        return value::encode_bool(false);
    }
    if !is_object_like(prototype) && !value::is_null(prototype) {
        ctx.set_last_error(
            "TypeError: Reflect.setPrototypeOf prototype must be an object or null".to_string(),
        );
        return value::encode_bool(false);
    }
    crate::proxy_reflect_async::reflect_set_prototype_of_fn_impl(ctx, target, prototype).await
}

pub async fn reflect_is_extensible<E: ExecContext>(ctx: &mut E, target: Value) -> Value {
    if !is_object_like(target) {
        ctx.set_last_error("TypeError: Reflect.isExtensible called on non-object".to_string());
        return value::encode_bool(false);
    }
    if !value::is_proxy(target) {
        return value::encode_bool(ctx.is_extensible(target));
    }
    let (target, handler) = match crate::proxy_traps::proxy_trap_proxy_entry(
        ctx,
        target,
        "isExtensible",
    ) {
        Ok(pair) => pair,
        Err(exception) => return exception,
    };
    if let Some(trap) = crate::proxy_traps::proxy_trap_handler_trap(
        ctx,
        handler,
        "isExtensible",
    ) {
        return match ctx.call_js_async(trap, handler, &[target]).await {
            Ok(result) => value::encode_bool(value::is_truthy(result)),
            Err(_) => value::encode_bool(false),
        };
    }
    value::encode_bool(ctx.is_extensible(target))
}

pub async fn reflect_prevent_extensions<E: ExecContext>(ctx: &mut E, target: Value) -> Value {
    if !is_object_like(target) {
        ctx.set_last_error("TypeError: Reflect.preventExtensions called on non-object".to_string());
        return value::encode_bool(false);
    }
    if !value::is_proxy(target) {
        return value::encode_bool(ctx.prevent_extensions(target));
    }
    let (target, handler) = match crate::proxy_traps::proxy_trap_proxy_entry(
        ctx,
        target,
        "preventExtensions",
    ) {
        Ok(pair) => pair,
        Err(exception) => return exception,
    };
    if let Some(trap) = crate::proxy_traps::proxy_trap_handler_trap(
        ctx,
        handler,
        "preventExtensions",
    ) {
        return match ctx.call_js_async(trap, handler, &[target]).await {
            Ok(result) => value::encode_bool(value::is_truthy(result)),
            Err(_) => value::encode_bool(false),
        };
    }
    value::encode_bool(ctx.prevent_extensions(target))
}

pub async fn reflect_define_property<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    property: Value,
    descriptor: Value,
) -> Value {
    if !value::is_proxy(target) {
        return value::encode_bool(ctx.define_property_or_throw(target, property, descriptor));
    }
    let (target, handler) = match crate::proxy_traps::proxy_trap_proxy_entry(
        ctx,
        target,
        "defineProperty",
    ) {
        Ok(pair) => pair,
        Err(exception) => return exception,
    };
    if let Some(trap) = crate::proxy_traps::proxy_trap_handler_trap(
        ctx,
        handler,
        "defineProperty",
    ) {
        return match ctx
            .call_js_async(trap, handler, &[target, property, descriptor])
            .await
        {
            Ok(result) => value::encode_bool(value::is_truthy(result)),
            Err(_) => value::encode_bool(false),
        };
    }
    value::encode_bool(ctx.define_property_or_throw(target, property, descriptor))
}

pub async fn reflect_own_keys<E: ExecContext>(ctx: &mut E, target: Value) -> Value {
    if value::is_proxy(target) {
        crate::proxy_reflect_async::proxy_own_keys_trap_async(ctx, target).await
    } else {
        crate::proxy_reflect::reflect_own_keys_impl(ctx, target)
    }
}

pub async fn proxy_apply<E: ExecContext>(
    ctx: &mut E,
    proxy: Value,
    this_value: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some(entry) = ctx.proxy_entry(value::decode_proxy_handle(proxy)) else {
        return value::encode_undefined();
    };
    if ctx.proxy_is_revoked(value::decode_proxy_handle(proxy)) {
        ctx.set_last_error("TypeError: Cannot perform call on a proxy that has been revoked".to_string());
        return value::encode_undefined();
    }
    if !ctx.is_callable(entry.target) {
        ctx.set_last_error("TypeError: Proxy target must be callable".to_string());
        return value::encode_undefined();
    }
    let args = shadow_args(ctx, args_base, args_count);
    let trap = ctx.read_data_property(entry.handler, "apply");
    if !value::is_undefined(trap) && !value::is_null(trap) {
        let args_array = args_array(ctx, &args);
        return match ctx
            .call_js_async(
                trap,
                entry.handler,
                &[entry.target, this_value, args_array],
            )
            .await
        {
            Ok(result) => result,
            Err(_) => {
                ctx.set_last_error("TypeError: Proxy apply trap failed".to_string());
                value::encode_undefined()
            }
        };
    }
    crate::proxy_reflect_async::reflect_apply_impl_async(ctx, entry.target, this_value, &args).await
}

pub async fn proxy_construct<E: ExecContext>(
    ctx: &mut E,
    proxy: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let Some(entry) = ctx.proxy_entry(value::decode_proxy_handle(proxy)) else {
        return value::encode_undefined();
    };
    if ctx.proxy_is_revoked(value::decode_proxy_handle(proxy)) {
        ctx.set_last_error(
            "TypeError: Cannot perform construct on a proxy that has been revoked".to_string(),
        );
        return value::encode_undefined();
    }
    if !ctx.is_callable(entry.target) {
        ctx.set_last_error("TypeError: Proxy target must be a constructor".to_string());
        return value::encode_undefined();
    }
    let args = shadow_args(ctx, args_base, args_count);
    let trap = ctx.read_data_property(entry.handler, "construct");
    if !value::is_undefined(trap) && !value::is_null(trap) {
        let args_array = args_array(ctx, &args);
        return match ctx
            .call_js_async(trap, entry.handler, &[entry.target, args_array, proxy])
            .await
        {
            Ok(result) if value::is_js_object(result) => result,
            Ok(_) => {
                ctx.set_last_error(
                    "TypeError: Proxy construct trap returned non-object".to_string(),
                );
                value::encode_undefined()
            }
            Err(error) => {
                ctx.set_last_error(format!("TypeError: Proxy construct trap failed: {error}"));
                value::encode_undefined()
            }
        };
    }
    crate::proxy_reflect_async::reflect_construct_impl_async(ctx, entry.target, &args, proxy).await
}

fn is_object_like(value: Value) -> bool {
    value::is_object(value)
        || value::is_array(value)
        || value::is_function(value)
        || value::is_proxy(value)
}

fn shadow_args<E: ExecContext>(ctx: &mut E, args_base: i32, args_count: i32) -> Vec<Value> {
    (0..args_count.max(0))
        .map(|index| ctx.read_shadow_arg(args_base, index as u32))
        .collect()
}

fn args_array<E: ExecContext>(ctx: &mut E, args: &[Value]) -> Value {
    let array = ctx.alloc_array(args.len() as u32);
    for (index, argument) in args.iter().copied().enumerate() {
        ctx.array_write_elem(array, index as u32, argument);
    }
    ctx.array_write_length(array, args.len() as u32);
    array
}
