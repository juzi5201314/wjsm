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
            if value::is_null(proto) {
                alloc_host_null_proto_object(&mut caller, &env, 0)
            } else {
                let o = alloc_host_object(&mut caller, &env, 0);
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
            }
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
                let mut source = read_shadow_arg(&mut caller, args_base, i);
                if value::is_undefined(source) || value::is_null(source) {
                    continue;
                }
                if !value::is_js_object(source) {
                    source = to_object(&mut caller, source);
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
                return make_type_error_exception(
                    &mut caller,
                    "TypeError: Object.setPrototypeOf called on non-object",
                );
            }
            if !value::is_js_object(proto) && !value::is_null(proto) {
                return make_type_error_exception(
                    &mut caller,
                    "TypeError: Object.setPrototypeOf prototype must be an object or null",
                );
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
                    return make_type_error_exception(
                        &mut caller,
                        "TypeError: Object.setPrototypeOf: object is not extensible",
                    );
                }
                return obj;
            }
            // Set prototype — 先做循环检测（§20.1.2.21 step 5-7）
            let proto_handle = proto_handle_from_value(&mut caller, proto);
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return obj;
            };
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return obj;
            };
            // 读取当前 proto handle，若与 new 相同则直接返回
            {
                let data = memory.data(&caller);
                if ptr + 4 <= data.len() {
                    let current = u32::from_le_bytes([
                        data[ptr],
                        data[ptr + 1],
                        data[ptr + 2],
                        data[ptr + 3],
                    ]);
                    if current == proto_handle {
                        return obj;
                    }
                }
            }
            // 循环检测：从 proto 开始遍历原型链，若遇到 obj 自身则抛 TypeError
            if !value::is_null(proto) && value::is_js_object(proto) {
                let mut current = proto_handle;
                let mut depth = 0u32;
                const MAX_PROTO_DEPTH: u32 = 1000;
                let obj_handle_raw = handle_index_of(&mut caller, obj) as u32;
                while current != 0xFFFF_FFFF && current != 0 && depth < MAX_PROTO_DEPTH {
                    if current == obj_handle_raw {
                        return make_type_error_exception(
                            &mut caller,
                            "Cyclic __proto__ value",
                        );
                    }
                    if current & 0x8000_0000 != 0 {
                        break; // proxy handle: 不走 obj_table，跳过
                    }
                    let Some(current_ptr) =
                        resolve_handle_idx(&mut caller, current as usize)
                    else {
                        break;
                    };
                    let data = memory.data(&caller);
                    if current_ptr + 4 > data.len() {
                        break;
                    }
                    current = u32::from_le_bytes([
                        data[current_ptr],
                        data[current_ptr + 1],
                        data[current_ptr + 2],
                        data[current_ptr + 3],
                    ]);
                    depth += 1;
                }
            }
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
            let name_id = if let Some(name_id) = symbol_value_to_name_id(prop) {
                name_id
            } else {
                let prop_name = match render_value(&mut caller, prop) {
                    Ok(name) => name,
                    Err(_) => return value::encode_undefined(),
                };
                let Some(name_id) = find_memory_c_string(&mut caller, &prop_name) else {
                    return value::encode_undefined();
                };
                name_id
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

    let object_has_own_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, prop: i64| -> i64 {
            if value::is_null(obj) || value::is_undefined(obj) {
                return make_type_error_exception(
                    &mut caller,
                    "TypeError: Cannot convert undefined or null to object",
                );
            }
            let boxed = if value::is_js_object(obj) {
                obj
            } else {
                to_object(&mut caller, obj)
            };
            let Some(ptr) = resolve_handle(&mut caller, boxed) else {
                return value::encode_bool(false);
            };
            let Some(name_id) = object_property_name_id_from_key(&mut caller, prop) else {
                return value::encode_bool(false);
            };
            let found = find_property_slot_by_name_id(&mut caller, ptr, name_id);
            value::encode_bool(found.is_some())
        },
    );
    linker.define(&mut store, "env", "object.has_own", object_has_own_fn)?;

    let object_freeze_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            if !value::is_js_object(obj) {
                return obj;
            }
            let _ = object_seal_or_freeze_impl(&mut caller, obj, true);
            obj
        },
    );
    linker.define(&mut store, "env", "object.freeze", object_freeze_fn)?;

    let object_seal_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            if !value::is_js_object(obj) {
                return obj;
            }
            let _ = object_seal_or_freeze_impl(&mut caller, obj, false);
            obj
        },
    );
    linker.define(&mut store, "env", "object.seal", object_seal_fn)?;

    let object_is_frozen_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            value::encode_bool(object_is_frozen_impl(&mut caller, obj))
        },
    );
    linker.define(&mut store, "env", "object.is_frozen", object_is_frozen_fn)?;

    let object_is_sealed_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            value::encode_bool(object_is_sealed_impl(&mut caller, obj))
        },
    );
    linker.define(&mut store, "env", "object.is_sealed", object_is_sealed_fn)?;

    Ok(())
}

fn object_property_name_id_from_key(
    caller: &mut Caller<'_, RuntimeState>,
    prop: i64,
) -> Option<u32> {
    if let Some(name_id) = symbol_value_to_name_id(prop) {
        return Some(name_id);
    }
    let prop_name = render_value(caller, prop).ok()?;
    find_memory_c_string(caller, &prop_name)
}

fn write_slot_flags(caller: &mut Caller<'_, RuntimeState>, slot_offset: usize, flags: i32) -> bool {
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return false;
    };
    let data = memory.data_mut(caller);
    if slot_offset + 8 > data.len() {
        return false;
    }
    data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    true
}

fn collect_own_property_slots(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
) -> Vec<(usize, i32)> {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return vec![];
    };
    let data = mem.data(caller);
    if obj_ptr + 16 > data.len() {
        return vec![];
    }
    if data[obj_ptr + 4] == wjsm_ir::HEAP_TYPE_ARRAY {
        return vec![];
    }
    let num_props = u32::from_le_bytes([
        data[obj_ptr + 12],
        data[obj_ptr + 13],
        data[obj_ptr + 14],
        data[obj_ptr + 15],
    ]) as usize;
    let mut slots = Vec::with_capacity(num_props);
    for i in 0..num_props {
        let slot_offset = obj_ptr + 16 + i * 32;
        if slot_offset + 32 > data.len() {
            break;
        }
        let flags = i32::from_le_bytes([
            data[slot_offset + 4],
            data[slot_offset + 5],
            data[slot_offset + 6],
            data[slot_offset + 7],
        ]);
        slots.push((slot_offset, flags));
    }
    slots
}

fn object_seal_or_freeze_impl(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    freeze: bool,
) -> bool {
    if !value::is_js_object(obj) {
        return false;
    }
    if !prevent_extensions_impl(caller, obj) {
        return false;
    }
    let Some(ptr) = resolve_handle(caller, obj) else {
        return false;
    };
    let slots = collect_own_property_slots(caller, ptr);
    for (slot_offset, flags) in slots {
        let mut new_flags = flags & !constants::FLAG_CONFIGURABLE;
        if freeze && (flags & constants::FLAG_IS_ACCESSOR) == 0 {
            new_flags &= !constants::FLAG_WRITABLE;
        }
        let _ = write_slot_flags(caller, slot_offset, new_flags);
    }
    true
}

fn object_is_sealed_impl(caller: &mut Caller<'_, RuntimeState>, obj: i64) -> bool {
    if !value::is_js_object(obj) {
        return true;
    }
    if is_extensible_impl(caller, obj) {
        return false;
    }
    let Some(ptr) = resolve_handle(caller, obj) else {
        return false;
    };
    collect_own_property_slots(caller, ptr)
        .iter()
        .all(|(_, flags)| (flags & constants::FLAG_CONFIGURABLE) == 0)
}

fn object_is_frozen_impl(caller: &mut Caller<'_, RuntimeState>, obj: i64) -> bool {
    if !object_is_sealed_impl(caller, obj) {
        return false;
    }
    let Some(ptr) = resolve_handle(caller, obj) else {
        return false;
    };
    collect_own_property_slots(caller, ptr)
        .iter()
        .all(|(_, flags)| {
            (flags & constants::FLAG_IS_ACCESSOR) != 0 || (flags & constants::FLAG_WRITABLE) == 0
        })
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
            .unwrap_or_else(|e| e.into_inner());
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
        value::decode_proxy_handle(proto) | 0x8000_0000
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
