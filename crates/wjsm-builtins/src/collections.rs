//! Map / Set / WeakMap / WeakSet / ArrayBuffer / Date 算法。
//!
//! 表操作走 ExecContext 原语；NativeCallable 枚举留在 host-wasm。

use crate::iterable_collect::{collect_constructor_iterable_values, map_entry_pair};
use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

fn collection_handle<E: ExecContext>(ctx: &mut E, this_val: Value, prop: &str) -> Option<u32> {
    if !value::is_object(this_val) {
        return None;
    }
    // 经 name_id 读取（intern 一次），避免每次调用的运行时字符串分配。
    let name_id = ctx.intern_property_key(prop);
    let h = ctx.get_property_by_name_id(this_val, name_id);
    if value::is_undefined(h) {
        return None;
    }
    Some(value::decode_f64(h) as u32)
}

fn install_methods<E: ExecContext>(ctx: &mut E, obj: Value, pairs: &[(&str, &str)]) {
    for &(name, kind) in pairs {
        let method = ctx.create_collection_method(kind);
        ctx.define_data_property(obj, name, method);
    }
}

/// Map 构造器
pub async fn map_constructor<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let handle = ctx.map_table_create();
    if !fill_map_from_iterable(ctx, handle, arg).await {
        ctx.release_unowned_map_entry(handle);
        return value::encode_undefined();
    }
    let obj = ctx.alloc_object(12);
    let owner = value::decode_object_handle(obj);
    ctx.bind_map_owner(handle, owner);
    ctx.define_data_property(obj, "__map_handle__", value::encode_f64(handle as f64));
    install_methods(
        ctx,
        obj,
        &[
            ("set", "map_set"),
            ("get", "map_get"),
            ("has", "map_has"),
            ("delete", "map_delete"),
            ("clear", "map_clear"),
            ("forEach", "map_for_each"),
            ("keys", "map_keys"),
            ("values", "map_values"),
            ("entries", "map_entries"),
        ],
    );
    // size 为 accessor（getter = NativeCallable Size）
    let size_getter = ctx.create_collection_method("map_size");
    let size_key = ctx.intern_property_key("size");
    let _ = ctx.define_accessor_property_with_flags(
        value::decode_object_handle(obj),
        size_key,
        size_getter,
        value::encode_undefined(),
        wjsm_ir::constants::FLAG_CONFIGURABLE as u32,
    );
    let entries_fn = ctx.create_collection_method("map_entries");
    let iter_key = wjsm_host::encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR);
    let _ = ctx.define_data_property_with_flags(
        value::decode_object_handle(obj),
        iter_key,
        entries_fn,
        (wjsm_ir::constants::FLAG_CONFIGURABLE | wjsm_ir::constants::FLAG_WRITABLE) as u32,
    );
    obj
}

/// 从 iterable 填充 Map 表（SameValueZero 去重）。
async fn fill_map_from_iterable<E: ExecContext>(ctx: &mut E, handle: u32, arg: Value) -> bool {
    let Some(values) = collect_constructor_iterable_values(ctx, arg).await else {
        return false;
    };
    for entry_val in values {
        let Some((key, val)) = map_entry_pair(ctx, entry_val) else {
            return false;
        };
        // map_set 内部已做 SameValueZero 去重
        ctx.map_set(handle, key, val);
    }
    true
}

/// Set 构造器
pub async fn set_constructor<E: ExecContext>(ctx: &mut E, arg: Value) -> Value {
    let handle = ctx.set_table_create();
    if !fill_set_from_iterable(ctx, handle, arg).await {
        ctx.release_unowned_set_entry(handle);
        return value::encode_undefined();
    }
    let obj = ctx.alloc_object(12);
    let owner = value::decode_object_handle(obj);
    ctx.bind_set_owner(handle, owner);
    ctx.define_data_property(obj, "__set_handle__", value::encode_f64(handle as f64));
    install_methods(
        ctx,
        obj,
        &[
            ("add", "set_add"),
            ("has", "set_has"),
            ("delete", "set_delete"),
            ("clear", "set_clear"),
            ("forEach", "set_for_each"),
            ("keys", "set_keys"),
            ("values", "set_values"),
            ("entries", "set_entries"),
        ],
    );
    let size_getter = ctx.create_collection_method("set_size");
    let size_key = ctx.intern_property_key("size");
    let _ = ctx.define_accessor_property_with_flags(
        value::decode_object_handle(obj),
        size_key,
        size_getter,
        value::encode_undefined(),
        wjsm_ir::constants::FLAG_CONFIGURABLE as u32,
    );
    let values_fn = ctx.create_collection_method("set_values");
    let iter_key = wjsm_host::encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR);
    let _ = ctx.define_data_property_with_flags(
        value::decode_object_handle(obj),
        iter_key,
        values_fn,
        (wjsm_ir::constants::FLAG_CONFIGURABLE | wjsm_ir::constants::FLAG_WRITABLE) as u32,
    );
    obj
}

/// 从 iterable 填充 Set 表（SameValueZero 去重）。
async fn fill_set_from_iterable<E: ExecContext>(ctx: &mut E, handle: u32, arg: Value) -> bool {
    let Some(values) = collect_constructor_iterable_values(ctx, arg).await else {
        return false;
    };
    for val in values {
        // set_add 内部已做 SameValueZero 去重
        ctx.set_add(handle, val);
    }
    true
}

pub fn map_proto_set<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    key: Value,
    val: Value,
) -> Value {
    let Some(handle) = collection_handle(ctx, this_val, "__map_handle__") else {
        ctx.set_last_error(
            "TypeError: Method Map.prototype.set called on incompatible receiver".to_string(),
        );
        return this_val;
    };
    ctx.map_set(handle, key, val);
    this_val
}

pub fn map_proto_get<E: ExecContext>(ctx: &mut E, this_val: Value, key: Value) -> Value {
    let Some(handle) = collection_handle(ctx, this_val, "__map_handle__") else {
        ctx.set_last_error(
            "TypeError: Method Map.prototype.get called on incompatible receiver".to_string(),
        );
        return value::encode_undefined();
    };
    ctx.map_get(handle, key)
        .unwrap_or_else(value::encode_undefined)
}

pub fn set_proto_add<E: ExecContext>(ctx: &mut E, this_val: Value, key: Value) -> Value {
    let Some(handle) = collection_handle(ctx, this_val, "__set_handle__") else {
        ctx.set_last_error(
            "TypeError: Method Set.prototype.add called on incompatible receiver".to_string(),
        );
        return this_val;
    };
    ctx.set_add(handle, key);
    this_val
}

pub fn set_proto_has<E: ExecContext>(ctx: &mut E, this_val: Value, key: Value) -> Value {
    // 直连 __set_handle__（不先试 __map_handle__，避免 Set 每 op 两次属性读取）。
    if let Some(h) = collection_handle(ctx, this_val, "__set_handle__") {
        return value::encode_bool(ctx.map_set_has(h, key, true));
    }
    ctx.set_last_error(
        "TypeError: Method Set.prototype.has called on incompatible receiver".to_string(),
    );
    value::encode_bool(false)
}

pub fn set_proto_delete<E: ExecContext>(ctx: &mut E, this_val: Value, key: Value) -> Value {
    // 直连 __set_handle__。
    if let Some(h) = collection_handle(ctx, this_val, "__set_handle__") {
        return value::encode_bool(ctx.map_set_delete(h, key, true));
    }
    value::encode_bool(false)
}

pub fn map_set_has<E: ExecContext>(ctx: &mut E, this_val: Value, key: Value) -> Value {
    if let Some(h) = collection_handle(ctx, this_val, "__map_handle__") {
        return value::encode_bool(ctx.map_set_has(h, key, false));
    }
    if let Some(h) = collection_handle(ctx, this_val, "__set_handle__") {
        return value::encode_bool(ctx.map_set_has(h, key, true));
    }
    ctx.set_last_error(
        "TypeError: Method Map/Set.prototype.has called on incompatible receiver".to_string(),
    );
    value::encode_bool(false)
}

pub fn map_set_delete<E: ExecContext>(ctx: &mut E, this_val: Value, key: Value) -> Value {
    if let Some(h) = collection_handle(ctx, this_val, "__map_handle__") {
        return value::encode_bool(ctx.map_set_delete(h, key, false));
    }
    if let Some(h) = collection_handle(ctx, this_val, "__set_handle__") {
        return value::encode_bool(ctx.map_set_delete(h, key, true));
    }
    value::encode_bool(false)
}

pub fn map_set_clear<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    if let Some(h) = collection_handle(ctx, this_val, "__map_handle__") {
        ctx.map_set_clear(h, false);
        return value::encode_undefined();
    }
    if let Some(h) = collection_handle(ctx, this_val, "__set_handle__") {
        ctx.map_set_clear(h, true);
        return value::encode_undefined();
    }
    value::encode_undefined()
}

pub fn map_set_get_size<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    if let Some(h) = collection_handle(ctx, this_val, "__map_handle__") {
        return value::encode_f64(ctx.map_set_size(h, false) as f64);
    }
    if let Some(h) = collection_handle(ctx, this_val, "__set_handle__") {
        return value::encode_f64(ctx.map_set_size(h, true) as f64);
    }
    value::encode_f64(0.0)
}

pub async fn map_set_for_each<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    let cb = ctx.read_shadow_arg(args_base, 0);
    if !ctx.is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        ctx.read_shadow_arg(args_base, 1)
    } else {
        value::encode_undefined()
    };
    let (handle, is_set) = if let Some(h) = collection_handle(ctx, this_val, "__map_handle__") {
        (h, false)
    } else if let Some(h) = collection_handle(ctx, this_val, "__set_handle__") {
        (h, true)
    } else {
        ctx.set_last_error(
            "TypeError: Method Map/Set.prototype.forEach called on incompatible receiver"
                .to_string(),
        );
        return value::encode_undefined();
    };
    let entries = ctx.map_set_entries_snapshot(handle, is_set);
    for (k, v) in entries {
        // Map: callback(value, key, map); Set: callback(value, value, set)
        let args = if is_set {
            [v, v, this_val]
        } else {
            [v, k, this_val]
        };
        if ctx.call_js_async(cb, this_arg, &args).await.is_err() {
            return value::encode_undefined();
        }
    }
    value::encode_undefined()
}

pub fn map_set_keys<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    if let Some(h) = collection_handle(ctx, this_val, "__map_handle__") {
        return ctx.create_map_set_iterator(h, false, 0);
    }
    if let Some(h) = collection_handle(ctx, this_val, "__set_handle__") {
        return ctx.create_map_set_iterator(h, true, 0);
    }
    value::encode_undefined()
}

pub fn map_set_values<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    if let Some(h) = collection_handle(ctx, this_val, "__map_handle__") {
        return ctx.create_map_set_iterator(h, false, 1);
    }
    if let Some(h) = collection_handle(ctx, this_val, "__set_handle__") {
        return ctx.create_map_set_iterator(h, true, 1);
    }
    value::encode_undefined()
}

pub fn map_set_entries<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    if let Some(h) = collection_handle(ctx, this_val, "__map_handle__") {
        return ctx.create_map_set_iterator(h, false, 2);
    }
    if let Some(h) = collection_handle(ctx, this_val, "__set_handle__") {
        return ctx.create_map_set_iterator(h, true, 2);
    }
    value::encode_undefined()
}

/// WeakMap 构造器
pub fn weakmap_constructor<E: ExecContext>(ctx: &mut E) -> Value {
    let handle = ctx.weakmap_table_create();
    let obj = ctx.alloc_object(5);
    ctx.define_data_property(obj, "__weakmap_handle__", value::encode_f64(handle as f64));
    install_methods(
        ctx,
        obj,
        &[
            ("set", "weakmap_set"),
            ("get", "weakmap_get"),
            ("has", "weakmap_has"),
            ("delete", "weakmap_delete"),
        ],
    );
    obj
}

pub fn weakmap_proto_set<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    key: Value,
    val: Value,
) -> Value {
    let Some(handle) = collection_handle(ctx, this_val, "__weakmap_handle__") else {
        return this_val;
    };
    let Some(key_h) = ctx.weak_target_handle(key) else {
        ctx.set_last_error("TypeError: WeakMap key must be an object".to_string());
        return this_val;
    };
    ctx.weakmap_set(handle, key_h, val);
    this_val
}

pub fn weakmap_proto_get<E: ExecContext>(ctx: &mut E, this_val: Value, key: Value) -> Value {
    let Some(handle) = collection_handle(ctx, this_val, "__weakmap_handle__") else {
        return value::encode_undefined();
    };
    let Some(key_h) = ctx.weak_target_handle(key) else {
        return value::encode_undefined();
    };
    ctx.weakmap_get(handle, key_h)
        .unwrap_or_else(value::encode_undefined)
}

pub fn weakmap_proto_has<E: ExecContext>(ctx: &mut E, this_val: Value, key: Value) -> Value {
    let Some(handle) = collection_handle(ctx, this_val, "__weakmap_handle__") else {
        return value::encode_bool(false);
    };
    let Some(key_h) = ctx.weak_target_handle(key) else {
        return value::encode_bool(false);
    };
    value::encode_bool(ctx.weakmap_has(handle, key_h))
}

pub fn weakmap_proto_delete<E: ExecContext>(ctx: &mut E, this_val: Value, key: Value) -> Value {
    let Some(handle) = collection_handle(ctx, this_val, "__weakmap_handle__") else {
        return value::encode_bool(false);
    };
    let Some(key_h) = ctx.weak_target_handle(key) else {
        return value::encode_bool(false);
    };
    value::encode_bool(ctx.weakmap_delete(handle, key_h))
}

/// WeakSet 构造器
pub fn weakset_constructor<E: ExecContext>(ctx: &mut E) -> Value {
    let handle = ctx.weakset_table_create();
    let obj = ctx.alloc_object(4);
    ctx.define_data_property(obj, "__weakset_handle__", value::encode_f64(handle as f64));
    install_methods(
        ctx,
        obj,
        &[
            ("add", "weakset_add"),
            ("has", "weakset_has"),
            ("delete", "weakset_delete"),
        ],
    );
    obj
}

pub fn weakset_proto_add<E: ExecContext>(ctx: &mut E, this_val: Value, key: Value) -> Value {
    let Some(handle) = collection_handle(ctx, this_val, "__weakset_handle__") else {
        return this_val;
    };
    let Some(key_h) = ctx.weak_target_handle(key) else {
        ctx.set_last_error("TypeError: WeakSet value must be an object".to_string());
        return this_val;
    };
    ctx.weakset_add(handle, key_h);
    this_val
}

pub fn weakset_proto_has<E: ExecContext>(ctx: &mut E, this_val: Value, key: Value) -> Value {
    let Some(handle) = collection_handle(ctx, this_val, "__weakset_handle__") else {
        return value::encode_bool(false);
    };
    let Some(key_h) = ctx.weak_target_handle(key) else {
        return value::encode_bool(false);
    };
    value::encode_bool(ctx.weakset_has(handle, key_h))
}

pub fn weakset_proto_delete<E: ExecContext>(ctx: &mut E, this_val: Value, key: Value) -> Value {
    let Some(handle) = collection_handle(ctx, this_val, "__weakset_handle__") else {
        return value::encode_bool(false);
    };
    let Some(key_h) = ctx.weak_target_handle(key) else {
        return value::encode_bool(false);
    };
    value::encode_bool(ctx.weakset_delete(handle, key_h))
}

/// ArrayBuffer 构造器
pub fn arraybuffer_constructor<E: ExecContext>(ctx: &mut E, length_val: Value) -> Value {
    let n = value::decode_f64(ctx.to_number(length_val));
    if !n.is_finite() || n < 0.0 {
        return ctx.make_range_error("Invalid array buffer length");
    }
    let byte_length = n.trunc() as u32;
    let Some(handle) = ctx.arraybuffer_create(byte_length) else {
        return value::encode_undefined();
    };
    let obj = ctx.alloc_object(3);
    ctx.define_data_property(
        obj,
        "__arraybuffer_handle__",
        value::encode_f64(handle as f64),
    );
    ctx.define_data_property(obj, "byteLength", value::encode_f64(byte_length as f64));
    obj
}

pub fn arraybuffer_proto_byte_length<E: ExecContext>(ctx: &mut E, this_val: Value) -> Value {
    let Some(handle) = collection_handle(ctx, this_val, "__arraybuffer_handle__") else {
        return value::encode_f64(0.0);
    };
    ctx.arraybuffer_byte_length(handle)
        .map(|n| value::encode_f64(n as f64))
        .unwrap_or_else(|| value::encode_f64(0.0))
}

pub fn arraybuffer_proto_slice<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    start: Value,
    end: Value,
) -> Value {
    let Some(handle) = collection_handle(ctx, this_val, "__arraybuffer_handle__") else {
        return value::encode_undefined();
    };
    let Some(len) = ctx.arraybuffer_byte_length(handle) else {
        return value::encode_undefined();
    };
    let s = to_rel_index(len, start, 0);
    let e = to_rel_index(len, end, len as i32);
    let Some(new_h) = ctx.arraybuffer_slice(handle, s, e) else {
        return value::encode_undefined();
    };
    let new_len = e.saturating_sub(s);
    let obj = ctx.alloc_object(3);
    ctx.define_data_property(
        obj,
        "__arraybuffer_handle__",
        value::encode_f64(new_h as f64),
    );
    ctx.define_data_property(obj, "byteLength", value::encode_f64(new_len as f64));
    obj
}

fn to_rel_index(length: u32, raw: Value, default: i32) -> u32 {
    if value::is_undefined(raw) {
        return default.max(0) as u32;
    }
    let f = value::decode_f64(raw);
    if f.is_nan() {
        return default.max(0) as u32;
    }
    let len = length as i32;
    let idx = if f < 0.0 {
        (len + f as i32).max(0)
    } else {
        (f as i32).min(len)
    };
    idx.max(0) as u32
}

// Date 构造器 / now / parse / utc 含 NativeCallable 方法表，保留在 host-wasm
// （`collections_buffers::define_collections_buffers`）。纯解析见 `date_parse`。
