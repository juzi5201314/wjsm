use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};

use crate::*;

pub(crate) fn define_object_builtins(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    // ── Object.is(a, b) → SameValue algorithm ─────────────────────────
    // type_idx 2: (i64, i64) -> i64
    let object_is_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let result = same_value(&mut caller, a, b);
            value::encode_bool(result)
        },
    );
    linker.define(&mut store, "env", "object.is", object_is_fn)?;

    // ── Object.create(proto, properties?) ─────────────────────────────
    // type_idx 2: (i64, i64) -> i64
    // Backend always passes 2 args; properties is encode_undefined() if absent.
    let object_create_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, proto: i64, _properties: i64| -> i64 {
            if !value::is_js_object(proto) && !value::is_null(proto) {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Object.create prototype may only be an object or null".to_string(),
                );
                return value::encode_undefined();
            }
            // Allocate new object; if proto is null, use null-proto allocator
            let env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
            let obj = if value::is_null(proto) {
                alloc_host_null_proto_object(&mut caller, &env, 0)
            } else {
                let o = alloc_host_object(&mut caller, &env, 0);
                // Set prototype
                let proto_handle = proto_handle_from_value(&mut caller, proto);
                if let Some(ptr) = resolve_handle(&mut caller, o) {
                    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                        return o;
                    };
                    let data = memory.data_mut(&mut caller);
                    if ptr + 4 <= data.len() {
                        data[ptr..ptr + 4].copy_from_slice(&proto_handle.to_le_bytes());
                    }
                }
                o
            };
            obj
        },
    );
    linker.define(&mut store, "env", "object.create", object_create_fn)?;

    // ── Object.assign(target, ...sources) ─────────────────────────────
    // type_idx 12: (i64, i64, i32, i32) -> i64 (variadic via shadow stack)
    let object_assign_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         target: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            if !value::is_js_object(target) {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Object.assign target must be an object".to_string(),
                );
                return value::encode_undefined();
            }
            for i in 0..args_count as u32 {
                let source = read_shadow_arg(&mut caller, args_base, i);
                if value::is_undefined(source) || value::is_null(source) {
                    continue;
                }
                if !value::is_js_object(source) {
                    continue;
                }
                let Some(source_ptr) = resolve_handle(&mut caller, source) else {
                    continue;
                };
                let names = collect_own_property_names(&mut caller, source_ptr, true);
                for name in &names {
                    // Read property value from source
                    let name_val = store_runtime_string(&caller, name.clone());
                    let prop_value = read_property_by_string_key(&mut caller, source, name_val);
                    // Define on target
                    let _ = define_host_data_property_from_caller(
                        &mut caller,
                        target,
                        name,
                        prop_value,
                    );
                }
            }
            target
        },
    );
    linker.define(&mut store, "env", "object.assign", object_assign_fn)?;

    // ── Object.values(obj) → array of enumerable own property values ──
    // type_idx 3: (i64) -> i64
    let object_values_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            if !value::is_js_object(obj) {
                return alloc_array(&mut caller, 0);
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return alloc_array(&mut caller, 0);
            };
            let vals = collect_own_property_values(&mut caller, ptr, true);
            let arr = alloc_array(&mut caller, vals.len() as u32);
            for (i, val) in vals.into_iter().enumerate() {
                set_array_elem(&mut caller, arr, i as i32, val);
            }
            arr
        },
    );
    linker.define(&mut store, "env", "object.values", object_values_fn)?;

    // ── Object.getOwnPropertySymbols(obj) → all own Symbol keys ───────
    // type_idx 3: (i64) -> i64
    let object_get_own_property_symbols_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            if !value::is_js_object(obj) {
                return alloc_array(&mut caller, 0);
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return alloc_array(&mut caller, 0);
            };
            let symbols = collect_own_property_key_values(&mut caller, ptr, true);
            let arr = alloc_array(&mut caller, symbols.len() as u32);
            for (i, symbol) in symbols.into_iter().enumerate() {
                set_array_elem(&mut caller, arr, i as i32, symbol);
            }
            arr
        },
    );
    linker.define(
        &mut store,
        "env",
        "object.get_own_property_symbols",
        object_get_own_property_symbols_fn,
    )?;

    // ── Object.setPrototypeOf(obj, proto) → obj ───────────────────────
    // type_idx 2: (i64, i64) -> i64
    let object_set_prototype_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, proto: i64| -> i64 {
            if !value::is_js_object(obj) {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Object.setPrototypeOf called on non-object".to_string(),
                );
                return obj;
            }
            if !value::is_js_object(proto) && !value::is_null(proto) {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Object.setPrototypeOf prototype must be an object or null"
                        .to_string(),
                );
                return obj;
            }
            // Check extensibility
            if !is_extensible_impl(&mut caller, obj) {
                // If non-extensible, only succeed if proto matches current
                let new_handle = proto_handle_from_value(&mut caller, proto);
                let Some(ptr) = resolve_handle(&mut caller, obj) else {
                    return obj;
                };
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return obj;
                };
                let data = memory.data(&caller);
                if ptr + 4 > data.len() {
                    return obj;
                }
                let current_handle =
                    u32::from_le_bytes([data[ptr], data[ptr + 1], data[ptr + 2], data[ptr + 3]]);
                if current_handle != new_handle {
                    set_runtime_error(
                        caller.data(),
                        "TypeError: Object.setPrototypeOf: object is not extensible".to_string(),
                    );
                }
                return obj;
            }
            // Set prototype
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return obj;
            };
            let proto_handle = proto_handle_from_value(&mut caller, proto);
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return obj;
            };
            let data = memory.data_mut(&mut caller);
            if ptr + 4 <= data.len() {
                data[ptr..ptr + 4].copy_from_slice(&proto_handle.to_le_bytes());
            }
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "object.set_prototype_of",
        object_set_prototype_of_fn,
    )?;

    // ── Object.getOwnPropertyDescriptor (NOT in backend yet; reserve import) ──
    // Register it so it doesn't fail linking, but it won't be called until backend emits it.
    let object_get_own_property_descriptor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64| -> i64 {
            if !value::is_js_object(target) {
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_handle(&mut caller, target) else {
                return value::encode_undefined();
            };
            let prop_name = match render_value(&mut caller, prop) {
                Ok(name) => name,
                Err(_) => return value::encode_undefined(),
            };
            let Some(name_id) = find_memory_c_string(&mut caller, &prop_name) else {
                return value::encode_undefined();
            };
            let Some((slot_offset, flags, val)) =
                find_property_slot_by_name_id(&mut caller, ptr, name_id)
            else {
                return value::encode_undefined();
            };
            let is_accessor = (flags & constants::FLAG_IS_ACCESSOR) != 0;
            let enumerable = (flags & constants::FLAG_ENUMERABLE) != 0;
            let configurable = (flags & constants::FLAG_CONFIGURABLE) != 0;
            let (getter_val, setter_val) = if is_accessor {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_undefined();
                };
                let data = memory.data(&caller);
                if slot_offset + 32 > data.len() {
                    return value::encode_undefined();
                }
                let g = i64::from_le_bytes(
                    data[slot_offset + 16..slot_offset + 24].try_into().unwrap(),
                );
                let s = i64::from_le_bytes(
                    data[slot_offset + 24..slot_offset + 32].try_into().unwrap(),
                );
                (g, s)
            } else {
                (value::encode_undefined(), value::encode_undefined())
            };
            let env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
            let desc = alloc_host_object(&mut caller, &env, 4);
            if is_accessor {
                let _ = define_host_data_property_from_caller(&mut caller, desc, "get", getter_val);
                let _ = define_host_data_property_from_caller(&mut caller, desc, "set", setter_val);
            } else {
                let _ = define_host_data_property_from_caller(&mut caller, desc, "value", val);
                let _ = define_host_data_property_from_caller(
                    &mut caller,
                    desc,
                    "writable",
                    value::encode_bool((flags & constants::FLAG_WRITABLE) != 0),
                );
            }
            let _ = define_host_data_property_from_caller(
                &mut caller,
                desc,
                "enumerable",
                value::encode_bool(enumerable),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                desc,
                "configurable",
                value::encode_bool(configurable),
            );
            desc
        },
    );
    linker.define(
        &mut store,
        "env",
        "object.get_own_property_descriptor",
        object_get_own_property_descriptor_fn,
    )?;

    Ok(())
}

// ── SameValue algorithm (ECMAScript 7.2.12) ───────────────────────────
// Differs from === in two ways: NaN === NaN is true, and +0 !== -0.
fn same_value(caller: &mut Caller<'_, RuntimeState>, a: i64, b: i64) -> bool {
    // If bit patterns are identical, values are SameValue
    if a == b {
        return true;
    }
    // Both f64?
    if value::is_f64(a) && value::is_f64(b) {
        let af = value::decode_f64(a);
        let bf = value::decode_f64(b);
        // NaN handling: both NaN → true
        if af.is_nan() && bf.is_nan() {
            return true;
        }
        // +0 vs -0: different bit patterns caught above (a == b check),
        // so if we get here and both are zero, one is +0 and other is -0 → false
        if af == 0.0 && bf == 0.0 {
            return false;
        }
        return af == bf;
    }
    // For strings, compare content
    if value::is_string(a) && value::is_string(b) {
        let a_str = get_string_value(caller, a);
        let b_str = get_string_value(caller, b);
        return a_str == b_str;
    }
    // For BigInt, compare values
    if value::is_bigint(a) && value::is_bigint(b) {
        let a_handle = value::decode_bigint_handle(a) as usize;
        let b_handle = value::decode_bigint_handle(b) as usize;
        if a_handle == b_handle {
            return true;
        }
        let table = caller
            .data()
            .bigint_table
            .lock()
            .expect("bigint_table mutex");
        return table
            .get(a_handle)
            .zip(table.get(b_handle))
            .map(|(x, y)| x == y)
            .unwrap_or(false);
    }
    // Different nanbox tags or different handles → not same
    false
}

/// Convert a prototype value to its raw handle representation for storage in object header.
///
/// 函数/闭包值的 low32 是函数表索引；其属性对象 handle 从 `__function_props_base` 起算，
/// 故存储到 header proto 字段时必须加上 base，否则读回时解析到错误对象。
fn proto_handle_from_value(caller: &mut Caller<'_, RuntimeState>, proto: i64) -> u32 {
    if value::is_null(proto) {
        0xFFFF_FFFF
    } else if value::is_object(proto) {
        value::decode_object_handle(proto)
    } else if value::is_array(proto) {
        value::decode_array_handle(proto)
    } else if value::is_proxy(proto) {
        value::decode_proxy_handle(proto)
    } else if value::is_function(proto) {
        let func_idx = value::decode_function_idx(proto);
        let base = caller
            .get_export("__function_props_base")
            .and_then(|e| e.into_global())
            .and_then(|g| g.get(&mut *caller).i32())
            .unwrap_or(0) as u32;
        base + func_idx
    } else if value::is_closure(proto) {
        let closure_idx = value::decode_closure_idx(proto) as usize;
        let func_idx = caller
            .data()
            .closures
            .lock()
            .ok()
            .and_then(|g| g.get(closure_idx).map(|e| e.func_idx))
            .unwrap_or(0);
        let base = caller
            .get_export("__function_props_base")
            .and_then(|e| e.into_global())
            .and_then(|g| g.get(&mut *caller).i32())
            .unwrap_or(0) as u32;
        base + func_idx
    } else {
        0xFFFF_FFFF
    }
}

/// Read a property from an object by string-key value (already encoded as runtime string).
fn read_property_by_string_key(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    key_val: i64,
) -> i64 {
    let key_str = match render_value(caller, key_val) {
        Ok(s) => s,
        Err(_) => return value::encode_undefined(),
    };
    let Some(ptr) = resolve_handle(caller, obj) else {
        return value::encode_undefined();
    };
    let Some(name_id) = find_memory_c_string(caller, &key_str) else {
        return value::encode_undefined();
    };
    let Some((_, _, val)) = find_property_slot_by_name_id(caller, ptr, name_id) else {
        return value::encode_undefined();
    };
    val
}
