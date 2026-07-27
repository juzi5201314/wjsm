//! Reflect + Object 同步属性算法（后端无关）。
//!
//! 纯算法在此；host-wasm 仅保留薄包装供未迁移文件（array_object.rs / core.rs）调用。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

/// 分配数据属性描述符对象 `{value, writable, enumerable, configurable}`。
pub fn alloc_data_property_descriptor<E: ExecContext>(
    ctx: &mut E,
    value_val: Value,
    writable: bool,
    enumerable: bool,
    configurable: bool,
) -> Value {
    let desc = ctx.alloc_object(4);
    ctx.define_data_property(desc, "value", value_val);
    ctx.define_data_property(desc, "writable", value::encode_bool(writable));
    ctx.define_data_property(desc, "enumerable", value::encode_bool(enumerable));
    ctx.define_data_property(desc, "configurable", value::encode_bool(configurable));
    desc
}

/// `Reflect.getOwnPropertyDescriptor(target, prop)` 同步路径（非 Proxy）。
///
/// 返回 undefined 或描述符对象。Proxy 路径在 `proxy_reflect_async` 中处理。
pub fn reflect_get_own_property_descriptor_impl<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    prop: Value,
) -> Value {
    // V2 堆路径
    if let Some(handle) = ctx.handle_index_of(target) {
        let Some(name_id) = ctx.property_value_to_name_id(prop, true) else {
            return value::encode_undefined();
        };
        let Some(slot) = ctx.get_own_property_slot(handle, name_id) else {
            return value::encode_undefined();
        };
        let (slot_val, flags, getter, setter) = slot;
        let is_accessor = (flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32) != 0;
        let enumerable = (flags & wjsm_ir::constants::FLAG_ENUMERABLE as u32) != 0;
        let configurable = (flags & wjsm_ir::constants::FLAG_CONFIGURABLE as u32) != 0;
        let desc = ctx.alloc_object(4);
        if is_accessor {
            ctx.define_data_property(desc, "get", getter);
            ctx.define_data_property(desc, "set", setter);
        } else {
            ctx.define_data_property(desc, "value", slot_val);
            ctx.define_data_property(
                desc,
                "writable",
                value::encode_bool((flags & wjsm_ir::constants::FLAG_WRITABLE as u32) != 0),
            );
        }
        ctx.define_data_property(desc, "enumerable", value::encode_bool(enumerable));
        ctx.define_data_property(desc, "configurable", value::encode_bool(configurable));
        return desc;
    }

    // Legacy memory32 路径已不再维护（V2 堆全覆盖）。
    value::encode_undefined()
}

/// `Reflect.ownKeys(target)` 同步路径（非 Proxy）。
///
/// 返回键数组。Proxy 路径在 `proxy_reflect_async::proxy_own_keys_trap_async` 中处理。
pub fn reflect_own_keys_impl<E: ExecContext>(ctx: &mut E, target: Value) -> Value {
    ctx.reflect_own_keys(target)
}

/// `Reflect.has(target, prop)` 同步路径（非 Proxy）。
pub fn reflect_has_impl<E: ExecContext>(ctx: &mut E, target: Value, prop: Value) -> Value {
    if !value::is_js_object(target) && !value::is_array(target) && !value::is_function(target) {
        return value::encode_bool(false);
    }
    if let Some(handle) = ctx.handle_index_of(target) {
        let Some(name_id) = ctx.property_value_to_name_id(prop, false) else {
            return value::encode_bool(false);
        };
        // 数组命名属性在 side table
        if value::is_array(target) && ctx.array_named_prop_get(target, name_id).is_some() {
            return value::encode_bool(true);
        }
        let found = ctx.get_property_slot_on_proto(handle, name_id).is_some();
        return value::encode_bool(found);
    }
    crate::core::ordinary_has_property(ctx, target, prop)
}

/// `delete property by name_id`（V2 堆路径）。
pub fn delete_property_by_name_id<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    name_id: u32,
) -> Value {
    let Some(handle) = ctx.handle_index_of(target) else {
        return value::encode_bool(true);
    };
    let Some(slot) = ctx.get_own_property_slot(handle, name_id) else {
        return value::encode_bool(true);
    };
    let (_val, flags, _getter, _setter) = slot;
    if (flags & wjsm_ir::constants::FLAG_CONFIGURABLE as u32) == 0 {
        return value::encode_bool(false);
    }
    // V2 删除：通过后端 delete_property_by_name_id 原语真正移除属性槽。
    let deleted = ctx.delete_property_by_name_id(handle, name_id);
    value::encode_bool(deleted)
}

/// `Reflect.deleteProperty(target, prop)` 同步路径（非 Proxy）。
pub fn reflect_delete_property_impl<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    prop: Value,
) -> Value {
    let prop_name = if value::is_string(prop) {
        ctx.read_string_utf8_lossy(prop)
    } else {
        ctx.value_to_display_string(prop)
    };
    // 数组元素删除必须在 name_id 解析前完成：数字索引可能尚未 intern 到
    // memory c-string 表，property_key_value_to_name_id(..., false) 会失败并
    // 误返回 true，导致 hole 从未写入。
    if value::is_array(target)
        && let Ok(index) = prop_name.parse::<u32>()
    {
        // 数组元素删除：写 hole（array_write_hole 是数组语义正确路径）
        ctx.array_write_hole(target, index);
        return value::encode_bool(true);
    }
    let Some(name_id) = ctx.property_value_to_name_id(prop, false) else {
        return value::encode_bool(true);
    };
    delete_property_by_name_id(ctx, target, name_id)
}
