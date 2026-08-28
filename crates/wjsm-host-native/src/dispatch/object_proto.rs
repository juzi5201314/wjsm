//! `%Object.prototype%` 上按需求值的原型方法：isPrototypeOf / toLocaleString
//! 与 Annex B 的 `__proto__` 访问器对、`__defineGetter__` / `__defineSetter__` /
//! `__lookupGetter__` / `__lookupSetter__`（ES §20.1.3、§B.2.2）。
//!
//! 这些函数对象在 `ensure_intrinsic_prototypes` 中一次性安装为
//! `%Object.prototype%` 的真实自有属性；本模块只负责调用期算法。

use std::collections::HashSet;

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::object;
use super::runtime::{
    fail_dispatch, get_property, object_handle, property_key, to_property_key_value, type_error,
};
use crate::{NativeAgentState, PropertyKey};

pub(super) fn dispatch_object_proto(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::ObjectProtoIsPrototypeOf => is_prototype_of(ctx, state, args),
        Builtin::ObjectProtoToLocaleString => to_locale_string(ctx, state, args),
        Builtin::ObjectProtoGetProto => proto_getter(ctx, state, args),
        Builtin::ObjectProtoSetProto => proto_setter(ctx, state, args),
        Builtin::ObjectProtoDefineGetter => define_accessor_member(ctx, state, args, true),
        Builtin::ObjectProtoDefineSetter => define_accessor_member(ctx, state, args, false),
        Builtin::ObjectProtoLookupGetter => lookup_accessor_member(ctx, state, args, true),
        Builtin::ObjectProtoLookupSetter => lookup_accessor_member(ctx, state, args, false),
        _ => return None,
    })
}

/// 规范意义上的 Object 值（本引擎里 TAG_REGEXP 独立于 is_js_object 之外）。
fn is_object_value(encoded: i64) -> bool {
    value::is_js_object(encoded) || value::is_regexp(encoded)
}

fn this_arg(args: &[i64]) -> i64 {
    args.first()
        .copied()
        .unwrap_or_else(value::encode_undefined)
}

fn nth_arg(args: &[i64], index: usize) -> i64 {
    args.get(index)
        .copied()
        .unwrap_or_else(value::encode_undefined)
}

/// `Object.prototype.isPrototypeOf`（§20.1.3.3）：步骤 1 的非对象 V 先于
/// ToObject(this) 短路；基元 this 的临时包装对象不可能出现在任何原型链上。
fn is_prototype_of(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let this = this_arg(args);
    let target = nth_arg(args, 1);
    if !is_object_value(target) {
        return value::encode_bool(false);
    }
    if value::is_null(this) || value::is_undefined(this) {
        return type_error(ctx, state, "Cannot convert undefined or null to object");
    }
    if !is_object_value(this) {
        return value::encode_bool(false);
    }
    // 步骤 3 从 V.[[GetPrototypeOf]]() 起查：自身不算自身的原型。
    let first = object::get_prototype(ctx, state, &[target]);
    if value::is_exception(first) {
        return first;
    }
    if value::is_null(first) {
        return value::encode_bool(false);
    }
    value::encode_bool(state.prototype_chain_contains_value(first, this))
}

/// `Object.prototype.toLocaleString`（§20.1.3.5）：Invoke(this, "toString")。
fn to_locale_string(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let this = this_arg(args);
    if value::is_null(this) || value::is_undefined(this) {
        return type_error(
            ctx,
            state,
            "Object.prototype.toLocaleString called on null or undefined",
        );
    }
    let Some(key) = state.intern_property_string("toString".into()) else {
        return fail_dispatch(ctx);
    };
    let Ok(method) = get_property(ctx, state, this, key.to_value()) else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(method) {
        return method;
    }
    if !value::is_callable(method) {
        let message = format!(
            "{} is not a function",
            non_callable_description(state, method)
        );
        return type_error(ctx, state, &message);
    }
    state
        .invoke_callable(ctx, method, this, &[])
        .unwrap_or_else(|| fail_dispatch(ctx))
}

/// V8 对 Invoke 命中非 callable 值的措辞（`number 1 is not a function` 等）。
fn non_callable_description(state: &mut NativeAgentState, encoded: i64) -> String {
    if value::is_f64(encoded) {
        return format!(
            "number {}",
            wjsm_builtins::format_number_js(value::decode_f64(encoded))
        );
    }
    if value::is_string(encoded) {
        let text = state
            .string_owned(encoded)
            .and_then(|text| text.to_utf8())
            .unwrap_or_default();
        return format!("string \"{text}\"");
    }
    if value::is_bool(encoded) {
        return format!("boolean {}", value::decode_bool(encoded));
    }
    if value::is_null(encoded) {
        return "object null".to_owned();
    }
    if value::is_undefined(encoded) {
        return "undefined".to_owned();
    }
    if value::is_bigint(encoded) {
        return "bigint".to_owned();
    }
    if value::is_symbol(encoded) {
        return "symbol".to_owned();
    }
    "object".to_owned()
}

/// `get Object.prototype.__proto__`（§B.2.2.1.1）：ToObject(this) 后取
/// [[GetPrototypeOf]]；本引擎基元 ToObject 的包装对象以 %Object.prototype%
/// 为 [[Prototype]]，故非对象 this 直接返回该固有原型。
fn proto_getter(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let this = this_arg(args);
    if value::is_null(this) || value::is_undefined(this) {
        return type_error(ctx, state, "Cannot convert undefined or null to object");
    }
    if is_object_value(this) {
        return object::get_prototype(ctx, state, &[this]);
    }
    state.object_prototype.unwrap_or_else(value::encode_null)
}

/// `set Object.prototype.__proto__`（§B.2.2.1.2）：proto 非对象/null 或 this
/// 为基元时静默返回 undefined；[[SetPrototypeOf]] 失败抛 TypeError。
fn proto_setter(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let this = this_arg(args);
    let proto = nth_arg(args, 1);
    if value::is_null(this) || value::is_undefined(this) {
        return type_error(
            ctx,
            state,
            "set Object.prototype.__proto__ called on null or undefined",
        );
    }
    if !is_object_value(proto) && !value::is_null(proto) {
        return value::encode_undefined();
    }
    if !is_object_value(this) {
        return value::encode_undefined();
    }
    let result = object::set_prototype(ctx, state, &[this, proto]);
    if value::is_exception(result) {
        return result;
    }
    value::encode_undefined()
}

/// `__defineGetter__` / `__defineSetter__`（§B.2.2.2 / §B.2.2.3）：构造
/// {get|set, enumerable: true, configurable: true} 描述符并复用
/// DefinePropertyOrThrow（含不可扩展/不可配置的拒绝路径）。
fn define_accessor_member(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    is_getter: bool,
) -> i64 {
    let this = this_arg(args);
    let key = nth_arg(args, 1);
    let accessor = nth_arg(args, 2);
    if value::is_null(this) || value::is_undefined(this) {
        return type_error(ctx, state, "Cannot convert undefined or null to object");
    }
    if !value::is_callable(accessor) {
        let message = if is_getter {
            "Object.prototype.__defineGetter__: Expecting function"
        } else {
            "Object.prototype.__defineSetter__: Expecting function"
        };
        return type_error(ctx, state, message);
    }
    let key = match to_property_key_value(ctx, state, key) {
        Ok(key) => key,
        Err(exception) => return exception,
    };
    // 基元 this 的 ToObject 包装对象即弃：副作用（ToPropertyKey）已发生。
    if !is_object_value(this) {
        return value::encode_undefined();
    }
    let Some(descriptor) = accessor_descriptor(ctx, state, accessor, is_getter) else {
        return fail_dispatch(ctx);
    };
    let result = object::define_property(ctx, state, &[this, key, descriptor]);
    if value::is_exception(result) {
        return result;
    }
    value::encode_undefined()
}

/// {get|set, enumerable: true, configurable: true} 描述符对象。
fn accessor_descriptor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    accessor: i64,
    is_getter: bool,
) -> Option<i64> {
    let descriptor = state.allocate_object_with_gc_retry(ctx, 3, false).ok()?;
    let handle = value::decode_handle(descriptor);
    let slot = if is_getter { "get" } else { "set" };
    for (name, stored) in [
        (slot, accessor),
        ("enumerable", value::encode_bool(true)),
        ("configurable", value::encode_bool(true)),
    ] {
        let key = state.intern_property_string(name.into())?;
        state
            .gc
            .heap()
            .set_property(handle, key, stored as u64)
            .ok()?;
    }
    Some(descriptor)
}

/// `__lookupGetter__` / `__lookupSetter__`（§B.2.2.4 / §B.2.2.5）：沿原型链
/// 找首个自有属性；访问器返回对应侧（可能是 undefined），数据属性终止查找。
fn lookup_accessor_member(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    want_getter: bool,
) -> i64 {
    let this = this_arg(args);
    let key = nth_arg(args, 1);
    if value::is_null(this) || value::is_undefined(this) {
        return type_error(ctx, state, "Cannot convert undefined or null to object");
    }
    let encoded_key = match to_property_key_value(ctx, state, key) {
        Ok(key) => key,
        Err(exception) => return exception,
    };
    let Some(key) = property_key(state, encoded_key) else {
        return fail_dispatch(ctx);
    };
    // 基元 this 的包装对象无自有属性，直接从 %Object.prototype% 起查。
    let mut current = if is_object_value(this) {
        this
    } else if let Some(prototype) = state.object_prototype {
        prototype
    } else {
        return value::encode_undefined();
    };
    let mut visited = HashSet::new();
    while visited.insert(current) {
        match own_property_kind(ctx, state, current, key, encoded_key) {
            OwnPropertyKind::Accessor { getter, setter } => {
                return if want_getter { getter } else { setter };
            }
            OwnPropertyKind::Data => return value::encode_undefined(),
            OwnPropertyKind::Exception(exception) => return exception,
            OwnPropertyKind::Missing => {}
        }
        let next = object::get_prototype(ctx, state, &[current]);
        if value::is_exception(next) {
            return next;
        }
        if !is_object_value(next) {
            return value::encode_undefined();
        }
        current = next;
    }
    value::encode_undefined()
}

enum OwnPropertyKind {
    Accessor { getter: i64, setter: i64 },
    Data,
    Missing,
    Exception(i64),
}

/// 单层 [[GetOwnProperty]] 的访问器判定：callable / 数组走旁挂表，proxy 走
/// trap 产出的描述符对象，普通堆对象读属性槽；regexp 的旁挂标志位（flags /
/// lastIndex 等）在本引擎不是可枚举的属性槽，按缺失处理沿链上行。
fn own_property_kind(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: PropertyKey,
    encoded_key: i64,
) -> OwnPropertyKind {
    if value::is_proxy(object) {
        return proxy_own_property_kind(ctx, state, object, encoded_key);
    }
    if value::is_callable(object) {
        let callable = value::strip_gc_color(object);
        let _ = state.callable_property(callable, key);
        if let Some((getter, setter)) = state.callable_accessors.get(&(callable, key)).copied() {
            return OwnPropertyKind::Accessor { getter, setter };
        }
        if state.callable_properties.contains_key(&(callable, key)) {
            return OwnPropertyKind::Data;
        }
        return OwnPropertyKind::Missing;
    }
    if value::is_array(object) {
        let handle = value::decode_handle(object);
        if let Some((getter, setter, _)) = state.array_accessors.get(&(handle, key)).copied() {
            return OwnPropertyKind::Accessor { getter, setter };
        }
        if state.array_properties.contains_key(&(handle, key))
            || state.text_matches(encoded_key, "length")
        {
            return OwnPropertyKind::Data;
        }
        if let Some(index) = super::runtime::array_index(state, encoded_key)
            && state
                .gc
                .heap()
                .get_element(handle, index)
                .ok()
                .flatten()
                .is_some_and(|element| !value::is_array_hole(element as i64))
        {
            return OwnPropertyKind::Data;
        }
        return OwnPropertyKind::Missing;
    }
    let Some(handle) = object_handle(object) else {
        return OwnPropertyKind::Missing;
    };
    match state.gc.heap().get_property_slot(handle, key) {
        Ok(Some(property)) => {
            if property.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0 {
                OwnPropertyKind::Accessor {
                    getter: property.getter as i64,
                    setter: property.setter as i64,
                }
            } else {
                OwnPropertyKind::Data
            }
        }
        Ok(None) => OwnPropertyKind::Missing,
        Err(_) => OwnPropertyKind::Exception(fail_dispatch(ctx)),
    }
}

/// proxy 层：经 [[GetOwnProperty]] trap 产出的描述符对象读取 get / set。
fn proxy_own_property_kind(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    encoded_key: i64,
) -> OwnPropertyKind {
    let descriptor = object::get_own_property_descriptor(ctx, state, &[object, encoded_key]);
    if value::is_exception(descriptor) {
        return OwnPropertyKind::Exception(descriptor);
    }
    if value::is_undefined(descriptor) {
        return OwnPropertyKind::Missing;
    }
    let handle = value::decode_handle(descriptor);
    let mut sides = [value::encode_undefined(), value::encode_undefined()];
    let mut is_accessor = false;
    for (index, name) in ["get", "set"].into_iter().enumerate() {
        let Some(key) = state.intern_property_string(name.into()) else {
            return OwnPropertyKind::Exception(fail_dispatch(ctx));
        };
        if let Ok(Some(property)) = state.gc.heap().get_property_slot(handle, key) {
            sides[index] = property.value as i64;
            is_accessor = true;
        }
    }
    if is_accessor {
        OwnPropertyKind::Accessor {
            getter: sides[0],
            setter: sides[1],
        }
    } else {
        OwnPropertyKind::Data
    }
}
