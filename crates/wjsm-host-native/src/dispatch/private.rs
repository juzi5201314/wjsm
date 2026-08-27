use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::{fail_dispatch, runtime};
use crate::{NativeAgentState, NativePrivateSlot, PropertyKey};

pub(super) fn dispatch_private(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::PrivateGet => get(ctx, state, args),
        Builtin::PrivateSet => set(ctx, state, args),
        Builtin::PrivateHas => has(ctx, state, args),
        Builtin::PrivateAccessorBind => bind_accessor(ctx, state, args),
        _ => return None,
    })
}

fn get(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, encoded_key] = args else {
        return fail_dispatch(ctx);
    };
    let Some(key) = private_key(*encoded_key) else {
        return fail_dispatch(ctx);
    };
    match state.private_slots.get(&(*receiver, key)).copied() {
        Some(NativePrivateSlot::Data(stored)) => stored,
        Some(NativePrivateSlot::Accessor { getter, .. }) if value::is_callable(getter) => state
            .invoke_callable(ctx, getter, *receiver, &[])
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Some(NativePrivateSlot::Accessor { .. }) => value::encode_undefined(),
        None => type_error(
            ctx,
            state,
            "Cannot read private member from an object whose class did not declare it",
        ),
    }
}

fn set(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, encoded_key, stored] = args else {
        return fail_dispatch(ctx);
    };
    let Some(key) = private_key(*encoded_key) else {
        return fail_dispatch(ctx);
    };
    match state.private_slots.get(&(*receiver, key)).copied() {
        Some(NativePrivateSlot::Accessor { setter, .. }) if value::is_callable(setter) => state
            .invoke_callable(ctx, setter, *receiver, &[*stored])
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Some(NativePrivateSlot::Accessor { .. }) => {
            type_error(ctx, state, "Private accessor was defined without a setter")
        }
        Some(NativePrivateSlot::Data(_)) => {
            state
                .private_slots
                .insert((*receiver, key), NativePrivateSlot::Data(*stored));
            *stored
        }
        None if establish_brand(state, *receiver, key) => {
            state
                .private_slots
                .insert((*receiver, key), NativePrivateSlot::Data(*stored));
            *stored
        }
        None => type_error(
            ctx,
            state,
            "Cannot write private member to an object whose class did not declare it",
        ),
    }
}

fn has(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, encoded_key, display_name] = args else {
        return fail_dispatch(ctx);
    };
    // async 状态机等不插表达式级分叉的上下文里，RHS 求值异常以 TAG_EXCEPTION
    // 流入本 builtin：原样透传，不得掩盖为 brand 检查的 TypeError。
    if value::is_exception(*receiver) {
        return *receiver;
    }
    // ES §13.10.1：`#x in rval` 的 rval 必须是对象（含函数 / 数组 / Proxy / RegExp
    // 等 exotic 对象），否则 TypeError；错误文案与 V8/Node 对齐，显示名由
    // lowering 传入（字段 `#x`，实例私有方法/访问器为类 brand 名）。
    if !(value::is_js_object(*receiver) || value::is_regexp(*receiver)) {
        let name = state.string_to_utf8(*display_name).unwrap_or_default();
        let rendered = runtime::render_value(state, *receiver);
        return type_error(
            ctx,
            state,
            &format!("Cannot use 'in' operator to search for '{name}' in {rendered}"),
        );
    }
    let Some(key) = private_key(*encoded_key) else {
        return fail_dispatch(ctx);
    };
    value::encode_bool(state.private_slots.contains_key(&(*receiver, key)))
}

fn bind_accessor(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, encoded_key, getter, setter] = args else {
        return fail_dispatch(ctx);
    };
    let Some(key) = private_key(*encoded_key) else {
        return fail_dispatch(ctx);
    };
    if !(value::is_callable(*getter) || value::is_undefined(*getter))
        || !(value::is_callable(*setter) || value::is_undefined(*setter))
        || !establish_brand(state, *receiver, key)
    {
        return fail_dispatch(ctx);
    }
    state.private_slots.insert(
        (*receiver, key),
        NativePrivateSlot::Accessor {
            getter: *getter,
            setter: *setter,
        },
    );
    value::encode_undefined()
}

fn establish_brand(state: &mut NativeAgentState, receiver: i64, key: PropertyKey) -> bool {
    let Some(brand) = receiver_brand(state, receiver) else {
        return false;
    };
    // 键必须用完整 64 位 PropertyKey：inline SSO 截断成 32 位会让不同类的
    // 同前缀私有名（如 `#x@1` 与 `#x@10`）碰撞，误判 brand 不匹配后静默丢槽。
    match state.private_brands.get(&key).copied() {
        Some(expected) => expected == brand,
        None => {
            state.private_brands.insert(key, brand);
            true
        }
    }
}

fn receiver_brand(state: &NativeAgentState, receiver: i64) -> Option<i64> {
    if value::is_callable(receiver) {
        return Some(receiver);
    }
    let handle = runtime::object_handle(receiver)?;
    let prototype = state.gc.heap().prototype(handle).ok()?;
    Some(if prototype == u32::MAX {
        value::encode_null()
    } else {
        value::encode_object_handle(prototype)
    })
}

fn private_key(encoded: i64) -> Option<PropertyKey> {
    PropertyKey::inline_string(encoded).or_else(|| {
        value::is_runtime_string_handle(encoded)
            .then(|| PropertyKey::from_name_id(value::decode_handle(encoded)))
    })
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    super::modules::named_error_object(state, "TypeError", message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
