//! Object 同步 builtins（算法在此；host 仅薄注册）。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::{constants, value};

/// ECMAScript 7.2.12 SameValue。
pub fn same_value<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> bool {
    if a == b {
        return true;
    }
    if value::is_f64(a) && value::is_f64(b) {
        let af = value::decode_f64(a);
        let bf = value::decode_f64(b);
        if af.is_nan() && bf.is_nan() {
            return true;
        }
        if af == 0.0 && bf == 0.0 {
            return false;
        }
        return af == bf;
    }
    if value::is_string(a) && value::is_string(b) {
        return ctx.get_runtime_string(a) == ctx.get_runtime_string(b);
    }
    if value::is_bigint(a) && value::is_bigint(b) {
        return match (ctx.read_bigint(a), ctx.read_bigint(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        };
    }
    false
}

/// SameValueZero（Array includes 等）。
pub fn same_value_zero<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> bool {
    if a == b {
        return true;
    }
    if value::is_f64(a) && value::is_f64(b) {
        let af = value::decode_f64(a);
        let bf = value::decode_f64(b);
        if af.is_nan() && bf.is_nan() {
            return true;
        }
        return af == bf;
    }
    if value::is_string(a) && value::is_string(b) {
        return ctx.get_runtime_string(a) == ctx.get_runtime_string(b);
    }
    if value::is_bigint(a) && value::is_bigint(b) {
        return match (ctx.read_bigint(a), ctx.read_bigint(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        };
    }
    false
}

pub fn object_is<E: ExecContext>(ctx: &mut E, a: Value, b: Value) -> Value {
    value::encode_bool(same_value(ctx, a, b))
}

pub fn object_create<E: ExecContext>(ctx: &mut E, proto: Value, properties: Value) -> Value {
    // ECMA-262 OrdinaryCreateFromConstructor / Object.create：
    // proto 必须是 Object 或 null；返回可捕获 TypeError（TAG_EXCEPTION）。
    if !value::is_undefined(proto) && !value::is_null(proto) && !value::is_js_object(proto) {
        return ctx.make_type_error("Object.create prototype may only be an object or null");
    }
    let o = if value::is_null(proto) {
        ctx.alloc_null_proto_object(0)
    } else {
        let o = ctx.alloc_object(0);
        if !value::is_undefined(proto) {
            ctx.set_object_proto(o, proto);
        }
        o
    };
    if !value::is_undefined(properties) {
        let result = object_define_properties(ctx, o, properties);
        if value::is_exception(result) {
            return result;
        }
    }
    o
}

pub fn object_assign<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if !value::is_js_object(target) {
        ctx.set_last_error("TypeError: Object.assign target must be an object".to_string());
        return value::encode_undefined();
    }
    for i in 0..args_count as u32 {
        let mut source = ctx.read_call_arg(
            wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
            i,
        );
        if value::is_undefined(source) || value::is_null(source) {
            continue;
        }
        if !value::is_js_object(source) {
            source = ctx.to_object(source);
        }
        let names = ctx.collect_own_property_names(source, true);
        for name in &names {
            let prop_value = ctx.read_property_by_string_key(source, name);
            ctx.define_data_property(target, name, prop_value);
        }
    }
    target
}

pub fn object_values<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    if !value::is_js_object(obj) {
        return ctx.alloc_array(0);
    }
    let names = ctx.collect_own_property_names(obj, true);
    let arr = ctx.alloc_array(names.len() as u32);
    for (i, name) in names.iter().enumerate() {
        let v = ctx.read_property_by_string_key(obj, name);
        ctx.array_write_elem(arr, i as u32, v);
    }
    arr
}

pub fn object_get_own_property_symbols<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    if !value::is_js_object(obj) {
        return ctx.alloc_array(0);
    }
    let symbols = ctx.collect_own_property_symbols(obj);
    let arr = ctx.alloc_array(symbols.len() as u32);
    for (i, sym) in symbols.into_iter().enumerate() {
        ctx.array_write_elem(arr, i as u32, sym);
    }
    arr
}

pub fn object_set_prototype_of<E: ExecContext>(ctx: &mut E, obj: Value, proto: Value) -> Value {
    if !value::is_js_object(obj) {
        return ctx.make_type_error("TypeError: Object.setPrototypeOf called on non-object");
    }
    if !value::is_js_object(proto) && !value::is_null(proto) {
        return ctx.make_type_error(
            "TypeError: Object.setPrototypeOf prototype must be an object or null",
        );
    }
    let new_handle = ctx.value_to_proto_handle(proto);
    if !ctx.is_extensible(obj) {
        let current = ctx.object_proto_handle(obj).unwrap_or(0xFFFF_FFFF);
        if current != new_handle {
            return ctx
                .make_type_error("TypeError: Object.setPrototypeOf: object is not extensible");
        }
        return obj;
    }
    if let Some(current) = ctx.object_proto_handle(obj)
        && current == new_handle
    {
        return obj;
    }
    // 环检测
    if !value::is_null(proto) && value::is_js_object(proto) {
        let obj_handle = ctx
            .handle_index_of(obj)
            .unwrap_or_else(|| value::decode_handle(obj));
        let mut current = new_handle;
        let mut depth = 0u32;
        const MAX_PROTO_DEPTH: u32 = 1000;
        while current != 0xFFFF_FFFF && current != 0 && depth < MAX_PROTO_DEPTH {
            if current == obj_handle {
                return ctx.make_type_error("Cyclic __proto__ value");
            }
            if current & 0x8000_0000 != 0 {
                break;
            }
            // 沿 proto 链：用 encode_handle_as_value 再读
            let as_val = ctx.encode_handle_as_value(current);
            current = ctx.object_proto_handle(as_val).unwrap_or(0xFFFF_FFFF);
            depth += 1;
        }
    }
    let _ = ctx.set_prototype_handle(obj, new_handle);
    obj
}

pub fn object_get_own_property_descriptor<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    prop: Value,
) -> Value {
    if !value::is_js_object(target) {
        return ctx.make_type_error("Object.getOwnPropertyDescriptor called on non-object");
    }
    ctx.get_own_property_descriptor_value(target, prop)
}

pub fn object_has_own<E: ExecContext>(ctx: &mut E, obj: Value, prop: Value) -> Value {
    if value::is_null(obj) || value::is_undefined(obj) {
        return ctx.make_type_error("TypeError: Cannot convert undefined or null to object");
    }
    let boxed = if value::is_js_object(obj) {
        obj
    } else {
        ctx.to_object(obj)
    };
    // allow_symbol=true：Object.hasOwn 必须支持 Symbol 键（perf_hooks brand 等）
    let Some(name_id) = ctx.property_value_to_name_id(prop, true) else {
        return value::encode_bool(false);
    };
    let Some(key) = ctx.canonicalize_name_id(name_id) else {
        return value::encode_bool(false);
    };
    let handle = if value::is_function(boxed) || value::is_closure(boxed) || value::is_bound(boxed)
    {
        ctx.handle_index_of(boxed)
            .unwrap_or_else(|| value::decode_handle(boxed))
    } else {
        value::decode_handle(boxed)
    };
    value::encode_bool(ctx.has_own_property_by_name_id(handle, key))
}

fn seal_or_freeze<E: ExecContext>(ctx: &mut E, obj: Value, freeze: bool) -> bool {
    if !value::is_js_object(obj) {
        return false;
    }
    if !ctx.prevent_extensions(obj) {
        return false;
    }
    let handle = ctx
        .handle_index_of(obj)
        .unwrap_or_else(|| value::decode_handle(obj));
    let entries = ctx.own_property_entries(handle);
    for (key, flags) in entries {
        let mut new_flags = flags & !(constants::FLAG_CONFIGURABLE as u32);
        if freeze && (flags & constants::FLAG_IS_ACCESSOR as u32) == 0 {
            new_flags &= !(constants::FLAG_WRITABLE as u32);
        }
        if !ctx.update_property_flags(handle, key, new_flags) {
            return false;
        }
    }
    true
}

pub fn object_freeze<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    if !value::is_js_object(obj) {
        return obj;
    }
    let _ = seal_or_freeze(ctx, obj, true);
    obj
}

pub fn object_seal<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    if !value::is_js_object(obj) {
        return obj;
    }
    let _ = seal_or_freeze(ctx, obj, false);
    obj
}

pub fn object_is_frozen<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    value::encode_bool(is_frozen(ctx, obj))
}

pub fn object_is_sealed<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    value::encode_bool(is_sealed(ctx, obj))
}

fn is_sealed<E: ExecContext>(ctx: &mut E, obj: Value) -> bool {
    if !value::is_js_object(obj) {
        return true;
    }
    if ctx.is_extensible(obj) {
        return false;
    }
    let handle = ctx
        .handle_index_of(obj)
        .unwrap_or_else(|| value::decode_handle(obj));
    ctx.own_property_entries(handle)
        .iter()
        .all(|(_, flags)| (flags & constants::FLAG_CONFIGURABLE as u32) == 0)
}

fn is_frozen<E: ExecContext>(ctx: &mut E, obj: Value) -> bool {
    if !is_sealed(ctx, obj) {
        return false;
    }
    if !value::is_js_object(obj) {
        return true;
    }
    let handle = ctx
        .handle_index_of(obj)
        .unwrap_or_else(|| value::decode_handle(obj));
    ctx.own_property_entries(handle).iter().all(|(_, flags)| {
        (flags & constants::FLAG_IS_ACCESSOR as u32) != 0
            || (flags & constants::FLAG_WRITABLE as u32) == 0
    })
}

pub fn object_define_properties<E: ExecContext>(ctx: &mut E, target: Value, props: Value) -> Value {
    if value::is_null(target) || value::is_undefined(target) {
        ctx.set_last_error("TypeError: Cannot convert undefined or null to object".to_string());
        return value::encode_undefined();
    }
    let boxed = if value::is_js_object(target) {
        target
    } else {
        ctx.to_object(target)
    };
    if value::is_undefined(props) {
        return boxed;
    }
    if !value::is_js_object(props) {
        ctx.set_last_error(
            "TypeError: Object.defineProperties properties must be an object".to_string(),
        );
        return value::encode_undefined();
    }
    let names = ctx.collect_own_property_names(props, false);
    for name in names {
        let key_val = ctx.store_string(&name);
        let desc = ctx.read_property_by_string_key(props, &name);
        if !ctx.define_property_or_throw(boxed, key_val, desc) {
            return value::encode_undefined();
        }
    }
    let symbols = ctx.collect_own_property_symbols(props);
    for sym in symbols {
        let desc = {
            // 按 symbol 键读：走 get_own_property_descriptor 的 prop 路径拿 value 不合适；
            // 用 name_id 读。
            let Some(name_id) = ctx.property_value_to_name_id(sym, true) else {
                ctx.set_last_error("TypeError: Invalid property key".to_string());
                return value::encode_undefined();
            };
            ctx.get_property_by_name_id(props, name_id)
        };
        if !ctx.define_property_or_throw(boxed, sym, desc) {
            return value::encode_undefined();
        }
    }
    boxed
}

/// `Object.getOwnPropertyDescriptors(obj)`：返回全部自有属性的描述符对象。
pub fn object_get_own_property_descriptors<E: ExecContext>(ctx: &mut E, target: Value) -> Value {
    if value::is_null(target) || value::is_undefined(target) {
        ctx.set_last_error("TypeError: Cannot convert undefined or null to object".to_string());
        return value::encode_undefined();
    }
    let object = if value::is_js_object(target) {
        target
    } else {
        ctx.to_object(target)
    };
    let keys = ctx.reflect_own_keys(object);
    let length = ctx.array_read_length(keys).unwrap_or(0);
    let result = ctx.alloc_object(0);
    for index in 0..length {
        let key = ctx
            .array_read_elem(keys, index)
            .unwrap_or_else(value::encode_undefined);
        let descriptor = ctx.get_own_property_descriptor_value(object, key);
        if value::is_undefined(descriptor) {
            continue;
        }
        if let Some(name_id) = ctx.property_value_to_name_id(key, true) {
            ctx.define_data_property_by_name_id(
                result,
                name_id,
                descriptor,
                constants::FLAG_WRITABLE
                    | constants::FLAG_CONFIGURABLE
                    | constants::FLAG_ENUMERABLE,
            );
        }
    }
    result
}

/// `Object.fromEntries(iterable)`：从 [key, value] 可迭代序列创建普通对象。
pub fn object_from_entries_impl<E: ExecContext>(ctx: &mut E, iterable: Value) -> Value {
    if value::is_null(iterable) || value::is_undefined(iterable) {
        ctx.set_last_error("TypeError: Cannot convert undefined or null to object".to_string());
        return value::encode_undefined();
    }
    let result = ctx.alloc_object(0);

    // 数组快速路径 + 通用可迭代路径统一走 collect_constructor_iterable_values
    let Some(values) = crate::iterable_collect::collect_constructor_iterable_values(ctx, iterable)
    else {
        ctx.set_last_error("TypeError: value is not iterable".to_string());
        return value::encode_undefined();
    };
    for entry_val in values {
        let Some((key_elem, val_elem)) = crate::iterable_collect::map_entry_pair(ctx, entry_val)
        else {
            ctx.set_last_error("TypeError: Iterator value is not an entry object".to_string());
            return value::encode_undefined();
        };
        if let Some(name_id) = ctx.property_value_to_name_id(key_elem, true) {
            ctx.define_data_property_by_name_id(
                result,
                name_id,
                val_elem,
                constants::FLAG_WRITABLE
                    | constants::FLAG_CONFIGURABLE
                    | constants::FLAG_ENUMERABLE,
            );
        }
    }
    result
}
