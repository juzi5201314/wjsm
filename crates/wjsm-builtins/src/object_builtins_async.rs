//! Object 再入异步 overrides。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

pub async fn obj_get_proto_of<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    if !value::is_js_object(obj) && !value::is_regexp(obj) {
        return value::encode_null();
    }
    ctx.object_get_prototype_of_async(obj).await
}

pub async fn object_is_extensible<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    if !value::is_js_object(obj) {
        return value::encode_bool(false);
    }
    value::encode_bool(ctx.object_is_extensible_async(obj).await)
}

pub async fn object_prevent_extensions<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    if !value::is_js_object(obj) {
        ctx.set_last_error(
            "TypeError: Object.preventExtensions called on non-object".to_string(),
        );
        return obj;
    }
    let result = ctx.object_prevent_extensions_async(obj).await;
    // 原逻辑：proxy trap 返回 falsy 且尚未有 runtime_error 时设置错误。
    // 通过 take 探测会丢消息；改为仅在成功路径外且为 proxy 时检查。
    // ExecContext 无 peek_last_error，用「先 take 再写回」保留消息。
    let prior = ctx.take_last_error();
    if !result && value::is_proxy(obj) && prior.is_none() {
        ctx.set_last_error(
            "TypeError: Object.preventExtensions proxy trap returned falsy".to_string(),
        );
    } else if let Some(msg) = prior {
        ctx.set_last_error(msg);
    }
    obj
}

pub async fn obj_keys<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    ctx.object_keys_async(obj).await
}

pub async fn obj_entries<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    ctx.object_entries_async(obj).await
}

pub async fn obj_values<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    ctx.object_values_async(obj).await
}

pub async fn obj_get_own_prop_names<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    ctx.object_get_own_property_names_async(obj).await
}

pub async fn obj_get_own_prop_symbols<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    ctx.object_get_own_property_symbols_async(obj).await
}

pub async fn obj_assign<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    ctx.object_assign_async(target, args_base, args_count).await
}
