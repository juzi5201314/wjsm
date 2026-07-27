//! core host builtins：运算符、枚举器、字符串拼接与属性入口。
//!
//! 复杂比较与 `instanceof` 分别放在子模块，避免把原 host 巨型文件原样搬迁。

mod equality;
mod instanceof;

pub use equality::{abstract_compare_impl, abstract_eq_impl, strict_eq_impl};
pub use instanceof::op_instanceof;

use wjsm_host::{ExecContext, ToPrimitiveHintKind, Value};
use wjsm_ir::value;

/// JavaScript remainder（与原 WASM import 的 raw f64 bits 契约一致）。
#[inline]
pub fn f64_mod(a: Value, b: Value) -> Value {
    let a = value::decode_f64(a);
    let b = value::decode_f64(b);
    (a - b * (a / b).trunc()).to_bits() as Value
}

/// JavaScript exponentiation（与原 WASM import 的 raw f64 bits 契约一致）。
#[inline]
pub fn f64_pow(a: Value, b: Value) -> Value {
    value::decode_f64(a).powf(value::decode_f64(b)).to_bits() as Value
}

/// ECMAScript §7.1.4 ToNumber，所有后端共享同一语义 owner。
#[inline]
pub fn to_number<E: ExecContext>(ctx: &mut E, input: Value) -> Value {
    if value::is_f64(input) {
        return input;
    }
    if value::is_undefined(input) {
        return value::encode_f64(f64::NAN);
    }
    if value::is_null(input) {
        return value::encode_f64(0.0);
    }
    if value::is_bool(input) {
        return value::encode_f64(f64::from(u8::from(value::decode_bool(input))));
    }
    if value::is_string(input) {
        let string = ctx.read_string_utf8_lossy(input);
        return value::encode_f64(crate::js_string_content_to_f64(&string));
    }
    if value::is_bigint(input) {
        return ctx.make_type_error("Cannot convert a BigInt value to a number");
    }
    if value::is_symbol(input) {
        return ctx.make_type_error("Cannot convert a Symbol value to a number");
    }
    if value::is_exception(input) {
        return input;
    }
    if value::is_js_object(input) || value::is_regexp(input) {
        let primitive = ctx.to_primitive_hinted(input, ToPrimitiveHintKind::Number);
        if value::is_exception(primitive) {
            return primitive;
        }
        return to_number(ctx, primitive);
    }
    value::encode_f64(f64::NAN)
}

#[inline]
pub fn iterator_value<E: ExecContext>(ctx: &mut E, handle: Value) -> Value {
    ctx.iterator_current_value(handle)
}

/// 兼容旧 WASM import 的未捕获异常报告路径。
pub fn legacy_throw<E: ExecContext>(ctx: &mut E, val: Value) {
    ctx.throw_exception(val);
    let rendered = ctx.render_value(val);
    let message = format!("Uncaught exception: {rendered}");
    ctx.write_output(format!("{message}\n").as_bytes());
    ctx.set_last_error(message);
}

pub fn create_exception<E: ExecContext>(ctx: &mut E, thrown_value: Value) -> Value {
    let rendered = ctx.render_value(thrown_value);
    ctx.push_exception("", &rendered, thrown_value)
}

#[inline]
pub fn exception_value<E: ExecContext>(ctx: &mut E, exception: Value) -> Value {
    ctx.exception_reason(exception)
}

#[inline]
pub fn enumerator_from<E: ExecContext>(ctx: &mut E, value: Value) -> Value {
    ctx.create_enumerator(value)
}

#[inline]
pub fn enumerator_next<E: ExecContext>(ctx: &mut E, handle: Value) -> Value {
    ctx.enumerator_advance(value::decode_handle(handle));
    value::encode_undefined()
}

#[inline]
pub fn enumerator_key<E: ExecContext>(ctx: &mut E, handle: Value) -> Value {
    ctx.enumerator_key(value::decode_handle(handle))
}

#[inline]
pub fn enumerator_done<E: ExecContext>(ctx: &mut E, handle: Value) -> Value {
    value::encode_bool(ctx.enumerator_done(value::decode_handle(handle)))
}
/// ECMAScript `typeof`，Proxy 按创建时的 [[Call]] 能力穿透已撤销条目。
#[inline]
pub fn typeof_impl<E: ExecContext>(ctx: &mut E, val: Value) -> Value {
    if value::is_undefined(val) {
        value::encode_typeof_undefined()
    } else if value::is_null(val) {
        value::encode_typeof_object()
    } else if value::is_bool(val) {
        value::encode_typeof_boolean()
    } else if value::is_string(val) {
        value::encode_typeof_string()
    } else if value::is_callable(val) {
        value::encode_typeof_function()
    } else if value::is_proxy(val) {
        let mut handle = value::decode_proxy_handle(val);
        let callable = loop {
            let Some(entry) = ctx.proxy_entry_any(handle) else {
                break false;
            };
            if value::is_callable(entry.target) {
                break true;
            }
            if !value::is_proxy(entry.target) {
                break false;
            }
            handle = value::decode_proxy_handle(entry.target);
        };
        if callable {
            value::encode_typeof_function()
        } else {
            value::encode_typeof_object()
        }
    } else if value::is_bigint(val) {
        value::encode_typeof_bigint()
    } else if value::is_symbol(val) {
        value::encode_typeof_symbol()
    } else if value::is_js_object(val)
        || value::is_iterator(val)
        || value::is_enumerator(val)
        || value::is_regexp(val)
    {
        value::encode_typeof_object()
    } else {
        value::encode_typeof_number()
    }
}

fn concat_operand_bytes<E: ExecContext>(ctx: &mut E, val: Value) -> Vec<u8> {
    if value::is_string(val) {
        return ctx.get_runtime_string(val).to_utf8_lossy_bytes();
    }
    if value::is_array(val) {
        return array_to_string_bytes(ctx, val);
    }
    if value::is_js_object(val) || value::is_regexp(val) {
        let primitive = ctx.to_primitive_hinted(val, ToPrimitiveHintKind::String);
        if value::is_exception(primitive) {
            return Vec::new();
        }
        return ctx.get_runtime_string(primitive).to_utf8_lossy_bytes();
    }
    ctx.render_value(val).into_bytes()
}

fn array_to_string_bytes<E: ExecContext>(ctx: &mut E, array: Value) -> Vec<u8> {
    let length = ctx.array_read_length(array).unwrap_or(0);
    let mut result = Vec::new();
    for index in 0..length {
        if index != 0 {
            result.push(b',');
        }
        let Some(element) = ctx.array_elem_at(array, index) else {
            continue;
        };
        if value::is_undefined(element) || value::is_null(element) {
            continue;
        }
        result.extend(concat_operand_bytes(ctx, element));
    }
    result
}
/// ECMAScript `+` 字符串拼接（非 Proxy 路径）。
#[inline]
pub fn string_concat<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    if !value::is_string(a) && !value::is_string(b) {
        return value::encode_undefined();
    }
    let mut bytes = concat_operand_bytes(ctx, a);
    bytes.extend(concat_operand_bytes(ctx, b));
    ctx.store_string_owned(String::from_utf8(bytes).unwrap_or_default())
}

pub fn string_concat_va<E: ExecContext>(ctx: &mut E, args_base: i32, args_count: i32) -> Value {
    let mut bytes = Vec::new();
    for index in 0..args_count as u32 {
        let arg = ctx.read_shadow_arg(args_base, index);
        bytes.extend(concat_operand_bytes(ctx, arg));
    }
    ctx.store_string_owned(String::from_utf8(bytes).unwrap_or_default())
}

/// 非 Proxy `in` 路径。Proxy trap 编排位于 `core_async::op_in`。
pub fn ordinary_has_property<E: ExecContext>(ctx: &mut E, object: Value, prop: Value) -> Value {
    if !value::is_js_object(object) {
        return ctx.make_type_error("cannot use 'in' operator on non-object");
    }
    let Some(name_id) = ctx.property_value_to_name_id(prop, true) else {
        return ctx.make_type_error("cannot convert property key");
    };

    if value::is_array(object) {
        if ctx.name_id_matches(name_id, "length") {
            return value::encode_bool(true);
        }
        if !value::is_symbol(prop) {
            let key = if value::is_string(prop) {
                ctx.get_runtime_string(prop).to_utf8_lossy()
            } else {
                ctx.value_to_display_string(prop)
            };
            if let Ok(index) = key.parse::<u32>() {
                return value::encode_bool(ctx.array_elem_at(object, index).is_some());
            }
        }
        if ctx.array_named_prop_get(object, name_id).is_some() {
            return value::encode_bool(true);
        }
    }

    let Some(handle) = ctx.handle_index_of(object) else {
        return value::encode_bool(false);
    };
    value::encode_bool(ctx.get_property_slot_on_proto(handle, name_id).is_some())
}

/// `Object.defineProperty` import 的 name_id 入口。
pub fn define_property_impl<E: ExecContext>(
    ctx: &mut E,
    object: Value,
    name_id: u32,
    descriptor: Value,
) -> Value {
    if !value::is_js_object(object) || value::is_proxy(object) {
        return ctx.make_type_error("Object.defineProperty called on non-object");
    }
    let Some(key) = ctx.name_id_to_property_key_value(name_id) else {
        return ctx.make_type_error("Invalid property key");
    };
    if ctx.define_property_or_throw(object, key, descriptor) {
        object
    } else {
        let message = ctx
            .take_last_error()
            .unwrap_or_else(|| "Cannot define property".to_string());
        ctx.make_type_error(&message)
    }
}

/// 同步 `get_own_prop_desc` import；回调通过 `call_js` 同步桥保持原签名。
pub fn get_own_prop_desc<E: ExecContext>(ctx: &mut E, object: Value, name_id: i32) -> Value {
    let prop = crate::proxy_traps::proxy_trap_property_key_value(ctx, name_id);
    if value::is_proxy(object) {
        let (target, handler) = match crate::proxy_traps::proxy_trap_proxy_entry(
            ctx,
            object,
            "getOwnPropertyDescriptor",
        ) {
            Ok(entry) => entry,
            Err(exception) => return exception,
        };
        let Some(trap) =
            crate::proxy_traps::proxy_trap_handler_trap(ctx, handler, "getOwnPropertyDescriptor")
        else {
            return crate::proxy_reflect::reflect_get_own_property_descriptor_impl(
                ctx, target, prop,
            );
        };
        // 同步 import 契约：仅 NativeCallable trap 可同步调用；JS trap 由异步路径承载。
        if !value::is_native_callable(trap) {
            return value::encode_undefined();
        }
        let descriptor = ctx.call_native_callable(trap, handler, &[target, prop]);
        if let Err(error) =
            crate::proxy_reflect_async::validate_proxy_get_own_property_descriptor_result(
                ctx,
                target,
                Some(name_id as u32),
                descriptor,
            )
        {
            ctx.set_last_error(error);
            return value::encode_undefined();
        }
        return descriptor;
    }
    if !value::is_js_object(object) && !value::is_regexp(object) {
        ctx.set_last_error(
            "TypeError: Object.getOwnPropertyDescriptor called on non-object".to_string(),
        );
        return value::encode_undefined();
    }
    ctx.get_own_property_descriptor_value(object, prop)
}
