use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

/// V2 `[[Get]]`：按规范化 name_id 分派原始值、数组、对象、函数与 Proxy。
pub async fn get_by_name_id<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    name_id: u32,
) -> Value {
    if value::is_proxy(receiver) {
        return crate::proxy_reflect_async::proxy_trap_internal_get_async(
            ctx,
            receiver,
            name_id as i32,
        )
        .await;
    }
    if value::is_native_callable(receiver) {
        return ctx.native_callable_get_property(receiver, name_id);
    }
    if value::is_array(receiver) {
        let Some(handle) = ctx.handle_index_of(receiver) else {
            return value::encode_undefined();
        };
        if ctx.name_id_matches(name_id, "length") {
            return ctx
                .array_length(handle)
                .map(|length| value::encode_f64(length as f64))
                .unwrap_or_else(value::encode_undefined);
        }
        if let Some((slot, flags, getter, _)) = ctx.get_own_property_slot(handle, name_id) {
            return read_property_slot(ctx, receiver, slot, flags, getter).await;
        }
        match ctx.lookup_property_on_proto(handle, name_id) {
            wjsm_host::PropertyLookup::Slot {
                value: slot,
                is_accessor,
                getter,
            } => return read_property(ctx, receiver, slot, is_accessor, getter).await,
            wjsm_host::PropertyLookup::Proxy(proxy) => {
                return crate::proxy_reflect_async::proxy_trap_internal_get_async(
                    ctx,
                    proxy,
                    name_id as i32,
                )
                .await;
            }
            wjsm_host::PropertyLookup::Missing => {}
        }
    }
    if value::is_regexp(receiver) {
        return ctx.primitive_regexp_get_property(receiver, name_id);
    }
    if value::is_symbol(receiver) {
        return ctx.primitive_symbol_get_property(receiver, name_id);
    }
    if value::is_bigint(receiver) {
        return crate::math_number_error::primitive_bigint_get_method(ctx, receiver, name_id);
    }
    if value::is_string(receiver) {
        return crate::string_methods::primitive_string_get_property(ctx, receiver, name_id);
    }
    if (receiver as u64 & value::BOX_BASE) != value::BOX_BASE {
        return crate::math_number_error::primitive_number_get_method(ctx, receiver, name_id);
    }
    if value::is_undefined(receiver) || value::is_null(receiver) {
        return value::encode_undefined();
    }
    if value::is_object(receiver)
        || value::is_function(receiver)
        || value::is_closure(receiver)
        || value::is_bound(receiver)
    {
        if let Some(handle) = ctx.handle_index_of(receiver)
            && ctx.resolve_handle(handle)
        {
            match ctx.lookup_property_on_proto(handle, name_id) {
                wjsm_host::PropertyLookup::Slot {
                    value: slot,
                    is_accessor,
                    getter,
                } => return read_property(ctx, receiver, slot, is_accessor, getter).await,
                wjsm_host::PropertyLookup::Proxy(proxy) => {
                    return crate::proxy_reflect_async::proxy_trap_internal_get_async(
                        ctx,
                        proxy,
                        name_id as i32,
                    )
                    .await;
                }
                wjsm_host::PropertyLookup::Missing => {}
            }
        }
        if value::is_function(receiver)
            || value::is_closure(receiver)
            || value::is_bound(receiver)
        {
            return ctx.callable_get_property(receiver, name_id);
        }
    }
    let Some(name) = property_name(ctx, name_id) else {
        return value::encode_undefined();
    };
    ctx.read_property_by_string_key(receiver, &name)
}

/// V2 `[[Set]]`：Proxy trap、RegExp side table 与 OrdinarySet 共用单一路径。
pub async fn set_by_name_id<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    name_id: u32,
    new_value: Value,
) {
    if value::is_proxy(receiver) {
        crate::proxy_reflect_async::proxy_trap_internal_set_async(
            ctx,
            receiver,
            name_id as i32,
            new_value,
        )
        .await;
        return;
    }
    if value::is_regexp(receiver) {
        ctx.primitive_regexp_set_property(receiver, name_id, new_value);
        return;
    }
    if value::is_array(receiver) && ctx.name_id_matches(name_id, "length") {
        let _ = crate::array_object::array_set_length(ctx, receiver, new_value);
        return;
    }
    if (value::is_function(receiver) || value::is_closure(receiver) || value::is_bound(receiver))
        && !ctx.ensure_property_storage(receiver)
    {
        return;
    }
    let _ = crate::proxy_reflect_async::ordinary_set_by_name_id(
        ctx,
        receiver,
        receiver,
        name_id,
        new_value,
    )
    .await;
}

/// V2 `[[Delete]]`：Proxy、数组索引 hole 与普通属性删除的统一入口。
pub async fn delete_by_name_id<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    name_id: u32,
) -> Value {
    if value::is_proxy(target) {
        return crate::proxy_reflect_async::proxy_trap_internal_delete_async(
            ctx,
            target,
            name_id as i32,
        )
        .await;
    }
    if value::is_array(target)
        && let Some(name) = property_name(ctx, name_id)
        && let Ok(index) = name.parse::<u32>()
    {
        ctx.array_write_hole(target, index);
        return value::encode_bool(true);
    }
    crate::proxy_reflect::delete_property_by_name_id(ctx, target, name_id)
}

async fn read_property_slot<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    slot: Value,
    flags: u32,
    getter: Value,
) -> Value {
    read_property(
        ctx,
        receiver,
        slot,
        flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0,
        getter,
    )
    .await
}

async fn read_property<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    slot: Value,
    is_accessor: bool,
    getter: Value,
) -> Value {
    if !is_accessor {
        return slot;
    }
    if value::is_undefined(getter) || value::is_null(getter) {
        return value::encode_undefined();
    }
    ctx.call_js_async(getter, receiver, &[])
        .await
        .unwrap_or_else(|_| value::encode_undefined())
}

fn property_name<E: ExecContext>(ctx: &mut E, name_id: u32) -> Option<String> {
    let key = ctx.name_id_to_property_key_value(name_id)?;
    if value::is_symbol(key) {
        None
    } else {
        ctx.value_to_key_string(key).ok()
    }
}
