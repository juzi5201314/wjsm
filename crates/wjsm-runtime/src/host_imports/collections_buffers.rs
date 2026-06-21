use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::*;

pub(crate) fn define_collections_buffers(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let map_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let handle;
            {
                let mut table = caller.data().map_table.lock().expect("map table mutex");
                table.push(MapEntry {
                    keys: Vec::new(),
                    values: Vec::new(),
                });
                handle = table.len() as u32 - 1;
            }
            // 处理可迭代参数：数组快速路径
            if !value::is_undefined(arg) && !value::is_null(arg)
                && value::is_array(arg)
                && let Some(arr_ptr) = resolve_handle(&mut caller, arg)
            {
                let len = read_array_length(&mut caller, arr_ptr).unwrap_or(0);
                let mut pairs: Vec<(i64, i64)> = Vec::new();
                for i in 0..len {
                    if let Some(entry_val) = read_array_elem(&mut caller, arr_ptr, i)
                        && value::is_array(entry_val)
                        && let Some(entry_ptr) = resolve_handle(&mut caller, entry_val)
                    {
                        let entry_len =
                            read_array_length(&mut caller, entry_ptr).unwrap_or(0);
                        if entry_len >= 2 {
                            let key = read_array_elem(&mut caller, entry_ptr, 0)
                                .unwrap_or_else(value::encode_undefined);
                            let val = read_array_elem(&mut caller, entry_ptr, 1)
                                .unwrap_or_else(value::encode_undefined);
                            pairs.push((key, val));
                        }
                    }
                }
                let mut table = caller.data().map_table.lock().expect("map table mutex");
                if (handle as usize) < table.len() {
                    let map_entry = &mut table[handle as usize];
                    for (key, val) in pairs {
                        let mut found = false;
                        for j in 0..map_entry.keys.len() {
                            if same_value_zero(map_entry.keys[j], key) {
                                map_entry.values[j] = val;
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            map_entry.keys.push(key);
                            map_entry.values.push(val);
                        }
                    }
                }
            }
            let (
                set_fn,
                get_fn,
                has_fn,
                delete_fn,
                clear_fn,
                size_fn,
                for_each_fn,
                keys_fn,
                values_fn,
                entries_fn,
            ) = {
                let state = caller.data();
                (
                    create_map_set_method(state, MapSetMethodKind::MapSet),
                    create_map_set_method(state, MapSetMethodKind::MapGet),
                    create_map_set_method(state, MapSetMethodKind::Has),
                    create_map_set_method(state, MapSetMethodKind::Delete),
                    create_map_set_method(state, MapSetMethodKind::Clear),
                    create_map_set_method(state, MapSetMethodKind::Size),
                    create_map_set_method(state, MapSetMethodKind::ForEach),
                    create_map_set_method(state, MapSetMethodKind::Keys),
                    create_map_set_method(state, MapSetMethodKind::Values),
                    create_map_set_method(state, MapSetMethodKind::Entries),
                )
            };
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 11)
            };
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__map_handle__",
                handle_val,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "set", set_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "get", get_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "has", has_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "delete", delete_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "clear", clear_fn);
            let _ = define_host_accessor_property_from_caller(
                &mut caller,
                obj,
                "size",
                size_fn,
                value::encode_undefined(),
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "forEach", for_each_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "keys", keys_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "values", values_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "entries", entries_fn);
            obj
        },
    );
    linker.define(&mut store, "env", "map_constructor", map_constructor_fn)?;

    let map_proto_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64, val: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr
                .and_then(|p| read_object_property_by_name(&mut caller, p, "__map_handle__"));
            let handle = handle_val
                .map(|v| value::decode_f64(v) as usize)
                .unwrap_or(0);
            let mut table = caller.data().map_table.lock().expect("map table mutex");
            if handle >= table.len() {
                return value::encode_undefined();
            }
            let entry = &mut table[handle];
            for i in 0..entry.keys.len() {
                if same_value_zero(entry.keys[i], key) {
                    entry.values[i] = val;
                    return this_val;
                }
            }
            entry.keys.push(key);
            entry.values.push(val);
            this_val
        },
    );
    linker.define(&mut store, "env", "map_proto_set", map_proto_set_fn)?;

    let map_proto_get_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr
                .and_then(|p| read_object_property_by_name(&mut caller, p, "__map_handle__"));
            let handle = handle_val
                .map(|v| value::decode_f64(v) as usize)
                .unwrap_or(0);
            let table = caller.data().map_table.lock().expect("map table mutex");
            if handle >= table.len() {
                return value::encode_undefined();
            }
            let entry = &table[handle];
            for i in 0..entry.keys.len() {
                if same_value_zero(entry.keys[i], key) {
                    return entry.values[i];
                }
            }
            value::encode_undefined()
        },
    );
    linker.define(&mut store, "env", "map_proto_get", map_proto_get_fn)?;

    // ── Set host functions ────────────────────────────────────────────
    let set_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let handle;
            {
                let mut table = caller.data().set_table.lock().expect("set table mutex");
                table.push(SetEntry { values: Vec::new() });
                handle = table.len() as u32 - 1;
            }
            // 处理可迭代参数：数组快速路径
            if !value::is_undefined(arg) && !value::is_null(arg)
                && value::is_array(arg)
                && let Some(arr_ptr) = resolve_handle(&mut caller, arg)
            {
                let len = read_array_length(&mut caller, arr_ptr).unwrap_or(0);
                let mut values: Vec<i64> = Vec::new();
                for i in 0..len {
                    if let Some(val) = read_array_elem(&mut caller, arr_ptr, i) {
                        let mut found = false;
                        for &v in &values {
                            if same_value_zero(v, val) {
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            values.push(val);
                        }
                    }
                }
                let mut table = caller.data().set_table.lock().expect("set table mutex");
                if (handle as usize) < table.len() {
                    let set_entry = &mut table[handle as usize];
                    for val in values {
                        let mut found = false;
                        for j in 0..set_entry.values.len() {
                            if same_value_zero(set_entry.values[j], val) {
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            set_entry.values.push(val);
                        }
                    }
                }
            }
            let (
                add_fn,
                has_fn,
                delete_fn,
                clear_fn,
                size_fn,
                for_each_fn,
                keys_fn,
                values_fn,
                entries_fn,
            ) = {
                let state = caller.data();
                (
                    create_map_set_method(state, MapSetMethodKind::SetAdd),
                    create_map_set_method(state, MapSetMethodKind::Has),
                    create_map_set_method(state, MapSetMethodKind::Delete),
                    create_map_set_method(state, MapSetMethodKind::Clear),
                    create_map_set_method(state, MapSetMethodKind::Size),
                    create_map_set_method(state, MapSetMethodKind::ForEach),
                    create_map_set_method(state, MapSetMethodKind::Keys),
                    create_map_set_method(state, MapSetMethodKind::Values),
                    create_map_set_method(state, MapSetMethodKind::Entries),
                )
            };
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 10)
            };
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__set_handle__",
                handle_val,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "add", add_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "has", has_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "delete", delete_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "clear", clear_fn);
            let _ = define_host_accessor_property_from_caller(
                &mut caller,
                obj,
                "size",
                size_fn,
                value::encode_undefined(),
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "forEach", for_each_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "keys", keys_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "values", values_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "entries", entries_fn);
            obj
        },
    );
    linker.define(&mut store, "env", "set_constructor", set_constructor_fn)?;

    let set_proto_add_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, val: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr
                .and_then(|p| read_object_property_by_name(&mut caller, p, "__set_handle__"));
            let handle = handle_val
                .map(|v| value::decode_f64(v) as usize)
                .unwrap_or(0);
            let mut table = caller.data().set_table.lock().expect("set table mutex");
            if handle >= table.len() {
                return value::encode_undefined();
            }
            let entry = &mut table[handle];
            for i in 0..entry.values.len() {
                if same_value_zero(entry.values[i], val) {
                    return this_val;
                }
            }
            entry.values.push(val);
            this_val
        },
    );
    linker.define(&mut store, "env", "set_proto_add", set_proto_add_fn)?;

    // ── Map/Set shared host functions (dispatch at runtime) ──────────
    let map_set_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_bool(false);
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            if let Some(op) = obj_ptr {
                let map_handle = read_object_property_by_name(&mut caller, op, "__map_handle__");
                if let Some(mh) = map_handle {
                    let handle = value::decode_f64(mh) as usize;
                    let table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        let entry = &table[handle];
                        for i in 0..entry.keys.len() {
                            if same_value_zero(entry.keys[i], key) {
                                return value::encode_bool(true);
                            }
                        }
                    }
                    return value::encode_bool(false);
                }
                let set_handle = read_object_property_by_name(&mut caller, op, "__set_handle__");
                if let Some(sh) = set_handle {
                    let handle = value::decode_f64(sh) as usize;
                    let table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        let entry = &table[handle];
                        for i in 0..entry.values.len() {
                            if same_value_zero(entry.values[i], key) {
                                return value::encode_bool(true);
                            }
                        }
                    }
                    return value::encode_bool(false);
                }
            }
            value::encode_bool(false)
        },
    );
    linker.define(&mut store, "env", "map_set_has", map_set_has_fn)?;

    let map_set_delete_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_bool(false);
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            if let Some(op) = obj_ptr {
                let map_handle = read_object_property_by_name(&mut caller, op, "__map_handle__");
                if let Some(mh) = map_handle {
                    let handle = value::decode_f64(mh) as usize;
                    let mut table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        let entry = &mut table[handle];
                        for i in 0..entry.keys.len() {
                            if same_value_zero(entry.keys[i], key) {
                                entry.keys.remove(i);
                                entry.values.remove(i);
                                return value::encode_bool(true);
                            }
                        }
                    }
                    return value::encode_bool(false);
                }
                let set_handle = read_object_property_by_name(&mut caller, op, "__set_handle__");
                if let Some(sh) = set_handle {
                    let handle = value::decode_f64(sh) as usize;
                    let mut table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        let entry = &mut table[handle];
                        for i in 0..entry.values.len() {
                            if same_value_zero(entry.values[i], key) {
                                entry.values.remove(i);
                                return value::encode_bool(true);
                            }
                        }
                    }
                    return value::encode_bool(false);
                }
            }
            value::encode_bool(false)
        },
    );
    linker.define(&mut store, "env", "map_set_delete", map_set_delete_fn)?;

    let map_set_clear_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            if let Some(op) = obj_ptr {
                let map_handle = read_object_property_by_name(&mut caller, op, "__map_handle__");
                if let Some(mh) = map_handle {
                    let handle = value::decode_f64(mh) as usize;
                    let mut table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        table[handle].keys.clear();
                        table[handle].values.clear();
                    }
                    return value::encode_undefined();
                }
                let set_handle = read_object_property_by_name(&mut caller, op, "__set_handle__");
                if let Some(sh) = set_handle {
                    let handle = value::decode_f64(sh) as usize;
                    let mut table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        table[handle].values.clear();
                    }
                    return value::encode_undefined();
                }
            }
            value::encode_undefined()
        },
    );
    linker.define(&mut store, "env", "map_set_clear", map_set_clear_fn)?;

    let map_set_get_size_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_f64(0.0);
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            if let Some(op) = obj_ptr {
                let map_handle = read_object_property_by_name(&mut caller, op, "__map_handle__");
                if let Some(mh) = map_handle {
                    let handle = value::decode_f64(mh) as usize;
                    let table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        return value::encode_f64(table[handle].keys.len() as f64);
                    }
                    return value::encode_f64(0.0);
                }
                let set_handle = read_object_property_by_name(&mut caller, op, "__set_handle__");
                if let Some(sh) = set_handle {
                    let handle = value::decode_f64(sh) as usize;
                    let table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        return value::encode_f64(table[handle].values.len() as f64);
                    }
                    return value::encode_f64(0.0);
                }
            }
            value::encode_f64(0.0)
        },
    );
    linker.define(&mut store, "env", "map_set_get_size", map_set_get_size_fn)?;

    let map_set_for_each_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64| -> i64 { value::encode_undefined() },
    );
    linker.define(&mut store, "env", "map_set_for_each", map_set_for_each_fn)?;

    let map_set_keys_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let map_handle = if let Some(ptr) = resolve_handle(&mut caller, this_val) {
                read_object_property_by_name(&mut caller, ptr, "__map_handle__")
                    .map(|v| value::decode_f64(v) as usize)
                    .unwrap_or(usize::MAX)
            } else {
                return value::encode_undefined();
            };
            let keys = {
                let table = caller.data().map_table.lock().expect("map table mutex");
                if let Some(entry) = table.get(map_handle) {
                    entry.keys.clone()
                } else {
                    return value::encode_undefined();
                }
            };
            let mut iters = caller.data().iterators.lock().expect("iterators mutex");
            let iter_handle = iters.len() as u32;
            iters.push(IteratorState::MapKeyIter { keys, index: 0 });
            value::encode_handle(value::TAG_ITERATOR, iter_handle)
        },
    );
    linker.define(&mut store, "env", "map_set_keys", map_set_keys_fn)?;

    let map_set_values_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64| -> i64 { value::encode_undefined() },
    );
    linker.define(&mut store, "env", "map_set_values", map_set_values_fn)?;

    let map_set_entries_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64| -> i64 { value::encode_undefined() },
    );
    linker.define(&mut store, "env", "map_set_entries", map_set_entries_fn)?;

    // ── WeakMap host functions ───────────────────────────────────────────
    let weakmap_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let handle;
            {
                let mut table = caller
                    .data()
                    .weakmap_table
                    .lock()
                    .expect("weakmap_table mutex");
                handle = table.len() as u32;
                table.push(WeakMapEntry {
                    map: HashMap::new(),
                });
            }
            let (set_fn, get_fn, has_fn, delete_fn) = {
                let state = caller.data();
                (
                    create_weakmap_method(state, WeakMapMethodKind::Set),
                    create_weakmap_method(state, WeakMapMethodKind::Get),
                    create_weakmap_method(state, WeakMapMethodKind::Has),
                    create_weakmap_method(state, WeakMapMethodKind::Delete),
                )
            };
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 5)
            };
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__weakmap_handle__",
                handle_val,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "set", set_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "get", get_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "has", has_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "delete", delete_fn);
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "weakmap_constructor",
        weakmap_constructor_fn,
    )?;

    let weakmap_proto_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64, val: i64| -> i64 {
            if !is_object_key(key) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Invalid value used as weak map key".to_string());
                return this_val;
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr
                .and_then(|p| read_object_property_by_name(&mut caller, p, "__weakmap_handle__"));
            let handle = handle_val
                .map(|v| value::decode_f64(v) as usize)
                .unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller
                    .data()
                    .weakmap_table
                    .lock()
                    .expect("weakmap_table mutex");
                if handle < table.len() {
                    table[handle].map.insert(key_handle, val);
                }
            }
            this_val
        },
    );
    linker.define(&mut store, "env", "weakmap_proto_set", weakmap_proto_set_fn)?;

    let weakmap_proto_get_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_undefined();
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr
                .and_then(|p| read_object_property_by_name(&mut caller, p, "__weakmap_handle__"));
            let handle = handle_val
                .map(|v| value::decode_f64(v) as usize)
                .unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller
                .data()
                .weakmap_table
                .lock()
                .expect("weakmap_table mutex");
            if handle < table.len()
                && let Some(&val) = table[handle].map.get(&key_handle)
            {
                return val;
            }
            value::encode_undefined()
        },
    );
    linker.define(&mut store, "env", "weakmap_proto_get", weakmap_proto_get_fn)?;

    let weakmap_proto_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr
                .and_then(|p| read_object_property_by_name(&mut caller, p, "__weakmap_handle__"));
            let handle = handle_val
                .map(|v| value::decode_f64(v) as usize)
                .unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller
                .data()
                .weakmap_table
                .lock()
                .expect("weakmap_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].map.contains_key(&key_handle));
            }
            value::encode_bool(false)
        },
    );
    linker.define(&mut store, "env", "weakmap_proto_has", weakmap_proto_has_fn)?;

    let weakmap_proto_delete_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr
                .and_then(|p| read_object_property_by_name(&mut caller, p, "__weakmap_handle__"));
            let handle = handle_val
                .map(|v| value::decode_f64(v) as usize)
                .unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let mut table = caller
                .data()
                .weakmap_table
                .lock()
                .expect("weakmap_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].map.remove(&key_handle).is_some());
            }
            value::encode_bool(false)
        },
    );
    linker.define(
        &mut store,
        "env",
        "weakmap_proto_delete",
        weakmap_proto_delete_fn,
    )?;

    // ── WeakSet host functions ───────────────────────────────────────────
    let weakset_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let handle;
            {
                let mut table = caller
                    .data()
                    .weakset_table
                    .lock()
                    .expect("weakset_table mutex");
                handle = table.len() as u32;
                table.push(WeakSetEntry {
                    set: HashSet::new(),
                });
            }
            let (add_fn, has_fn, delete_fn) = {
                let state = caller.data();
                (
                    create_weakset_method(state, WeakSetMethodKind::Add),
                    create_weakset_method(state, WeakSetMethodKind::Has),
                    create_weakset_method(state, WeakSetMethodKind::Delete),
                )
            };
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 4)
            };
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__weakset_handle__",
                handle_val,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "add", add_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "has", has_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "delete", delete_fn);
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "weakset_constructor",
        weakset_constructor_fn,
    )?;

    let weakset_proto_add_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Invalid value used in weak set".to_string());
                return this_val;
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr
                .and_then(|p| read_object_property_by_name(&mut caller, p, "__weakset_handle__"));
            let handle = handle_val
                .map(|v| value::decode_f64(v) as usize)
                .unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller
                    .data()
                    .weakset_table
                    .lock()
                    .expect("weakset_table mutex");
                if handle < table.len() {
                    table[handle].set.insert(key_handle);
                }
            }
            this_val
        },
    );
    linker.define(&mut store, "env", "weakset_proto_add", weakset_proto_add_fn)?;

    let weakset_proto_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr
                .and_then(|p| read_object_property_by_name(&mut caller, p, "__weakset_handle__"));
            let handle = handle_val
                .map(|v| value::decode_f64(v) as usize)
                .unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller
                .data()
                .weakset_table
                .lock()
                .expect("weakset_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].set.contains(&key_handle));
            }
            value::encode_bool(false)
        },
    );
    linker.define(&mut store, "env", "weakset_proto_has", weakset_proto_has_fn)?;

    let weakset_proto_delete_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr
                .and_then(|p| read_object_property_by_name(&mut caller, p, "__weakset_handle__"));
            let handle = handle_val
                .map(|v| value::decode_f64(v) as usize)
                .unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let mut table = caller
                .data()
                .weakset_table
                .lock()
                .expect("weakset_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].set.remove(&key_handle));
            }
            value::encode_bool(false)
        },
    );
    linker.define(
        &mut store,
        "env",
        "weakset_proto_delete",
        weakset_proto_delete_fn,
    )?;

    let arraybuffer_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, byte_length: i64| -> i64 {
            let len_f64 = value::decode_f64(byte_length);
            if len_f64 < 0.0 {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("RangeError: Invalid array buffer length".to_string());
                return value::encode_undefined();
            }
            let len = len_f64 as u32;
            let handle;
            {
                let mut table = caller
                    .data()
                    .arraybuffer_table
                    .lock()
                    .expect("arraybuffer_table mutex");
                handle = table.len() as u32;
                table.push(ArrayBufferEntry {
                    data: vec![0; len as usize],
                });
            }
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 4)
            };
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__arraybuffer_handle__",
                handle_val,
            );
            let bl_val = value::encode_f64(len as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteLength", bl_val);
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "arraybuffer_constructor",
        arraybuffer_constructor_fn,
    )?;

    let arraybuffer_proto_byte_length_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => match read_object_property_by_name(&mut caller, ptr, "byteLength") {
                    Some(v) => v,
                    None => value::encode_f64(0.0),
                },
                None => value::encode_f64(0.0),
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "arraybuffer_proto_byte_length",
        arraybuffer_proto_byte_length_fn,
    )?;

    let arraybuffer_proto_slice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, begin: i64, end: i64| -> i64 {
            let begin_idx = value::decode_f64(begin) as u32;
            let end_idx = value::decode_f64(end) as u32;
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let (buf_handle, buf_len) = match obj_ptr {
                Some(ptr) => {
                    let h =
                        read_object_property_by_name(&mut caller, ptr, "__arraybuffer_handle__");
                    let bl = read_object_property_by_name(&mut caller, ptr, "byteLength");
                    match (h, bl) {
                        (Some(hv), Some(lv)) => {
                            (value::decode_f64(hv) as u32, value::decode_f64(lv) as u32)
                        }
                        _ => return value::encode_undefined(),
                    }
                }
                None => return value::encode_undefined(),
            };
            let start = begin_idx.min(buf_len);
            let stop = end_idx.min(buf_len);
            let new_len = stop.saturating_sub(start);
            let new_buf_handle;
            {
                let mut ab_table = caller
                    .data()
                    .arraybuffer_table
                    .lock()
                    .expect("arraybuffer_table mutex");
                new_buf_handle = ab_table.len() as u32;
                let mut new_data = vec![0u8; new_len as usize];
                if let Some(buf_entry) = ab_table.get(buf_handle as usize) {
                    new_data.copy_from_slice(&buf_entry.data[start as usize..stop as usize]);
                }
                ab_table.push(ArrayBufferEntry { data: new_data });
            }
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 4)
            };
            let handle_val = value::encode_f64(new_buf_handle as f64);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__arraybuffer_handle__",
                handle_val,
            );
            let bl_val = value::encode_f64(new_len as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteLength", bl_val);
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "arraybuffer_proto_slice",
        arraybuffer_proto_slice_fn,
    )?;

    let dataview_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         buffer: i64,
         byte_offset: i64,
         byte_length: i64|
         -> i64 {
            let (buf_handle, buf_byte_length, is_shared) =
                match crate::shared_buffer::resolve_buffer_backing(&mut caller, buffer) {
                    Some(crate::shared_buffer::BufferBacking::SharedArrayBuffer {
                        handle,
                        byte_length,
                        ..
                    }) => (handle, byte_length, true),
                    Some(crate::shared_buffer::BufferBacking::ArrayBuffer {
                        handle,
                        byte_length,
                    }) => (handle, byte_length, false),
                    None => return value::encode_undefined(),
                };
            let offset = if value::is_undefined(byte_offset) {
                0
            } else {
                value::decode_f64(byte_offset) as u32
            };
            let length = if value::is_undefined(byte_length) {
                buf_byte_length.saturating_sub(offset)
            } else {
                value::decode_f64(byte_length) as u32
            };
            let handle;
            {
                let mut table = caller
                    .data()
                    .dataview_table
                    .lock()
                    .expect("dataview_table mutex");
                handle = table.len() as u32;
                table.push(DataViewEntry {
                    buffer_handle: buf_handle,
                    byte_offset: offset,
                    byte_length: length,
                    is_shared,
                });
            }
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 4)
            };
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__dataview_handle__",
                handle_val,
            );
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "dataview_constructor",
        dataview_constructor_fn,
    )?;

    macro_rules! dataview_get_fn {
        ($name:ident, $import:literal, $size:expr, $conv:expr) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, this_val: i64, byte_offset: i64| -> i64 {
                    let offset = value::decode_f64(byte_offset) as u32;
                    let obj_ptr = resolve_handle_idx(
                        &mut caller,
                        value::decode_object_handle(this_val) as usize,
                    );
                    let dv_handle = match obj_ptr {
                        Some(ptr) => {
                            match read_object_property_by_name(
                                &mut caller,
                                ptr,
                                "__dataview_handle__",
                            ) {
                                Some(v) => value::decode_f64(v) as usize,
                                None => return value::encode_undefined(),
                            }
                        }
                        None => return value::encode_undefined(),
                    };
                    let (buf_handle, dv_offset, dv_length, is_shared) = {
                        let dv_table = caller
                            .data()
                            .dataview_table
                            .lock()
                            .expect("dataview_table mutex");
                        if dv_handle < dv_table.len() {
                            let e = &dv_table[dv_handle];
                            (e.buffer_handle, e.byte_offset, e.byte_length, e.is_shared)
                        } else {
                            return value::encode_undefined();
                        }
                    };
                    let abs_offset = dv_offset as usize + offset as usize;
                    if offset + $size as u32 > dv_length {
                        *caller.data().runtime_error.lock().expect("error mutex") = Some(
                            "RangeError: Offset is outside the bounds of the DataView".to_string(),
                        );
                        return value::encode_undefined();
                    }
                    let mut bytes = [0u8; 8];
                    if !crate::shared_buffer::dataview_read_bytes(
                        &mut caller,
                        buf_handle,
                        is_shared,
                        abs_offset,
                        &mut bytes[..$size],
                    ) {
                        *caller.data().runtime_error.lock().expect("error mutex") = Some(
                            "RangeError: Offset is outside the bounds of the DataView".to_string(),
                        );
                        return value::encode_undefined();
                    }
                    return $conv(&bytes[..$size]);
                },
            );
            linker.define(&mut store, "env", $import, $name)?;
        };
    }

    dataview_get_fn!(
        dataview_proto_get_int8_fn,
        "dataview_proto_get_int8",
        1,
        |bytes: &[u8]| value::encode_f64(bytes[0] as i8 as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_uint8_fn,
        "dataview_proto_get_uint8",
        1,
        |bytes: &[u8]| value::encode_f64(bytes[0] as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_int16_fn,
        "dataview_proto_get_int16",
        2,
        |bytes: &[u8]| value::encode_f64(i16::from_le_bytes([bytes[0], bytes[1]]) as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_uint16_fn,
        "dataview_proto_get_uint16",
        2,
        |bytes: &[u8]| value::encode_f64(u16::from_le_bytes([bytes[0], bytes[1]]) as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_int32_fn,
        "dataview_proto_get_int32",
        4,
        |bytes: &[u8]| value::encode_f64(i32::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3]
        ]) as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_uint32_fn,
        "dataview_proto_get_uint32",
        4,
        |bytes: &[u8]| value::encode_f64(u32::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3]
        ]) as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_float32_fn,
        "dataview_proto_get_float32",
        4,
        |bytes: &[u8]| value::encode_f64(f32::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3]
        ]) as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_float64_fn,
        "dataview_proto_get_float64",
        8,
        |bytes: &[u8]| f64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]
        ])
        .to_bits() as i64
    );

    macro_rules! dataview_set_fn {
        ($name:ident, $import:literal, $size:expr, $write:expr) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>,
                 this_val: i64,
                 byte_offset: i64,
                 value_arg: i64|
                 -> i64 {
                    let offset = value::decode_f64(byte_offset) as u32;
                    let obj_ptr = resolve_handle_idx(
                        &mut caller,
                        value::decode_object_handle(this_val) as usize,
                    );
                    let dv_handle = match obj_ptr {
                        Some(ptr) => {
                            match read_object_property_by_name(
                                &mut caller,
                                ptr,
                                "__dataview_handle__",
                            ) {
                                Some(v) => value::decode_f64(v) as usize,
                                None => return value::encode_undefined(),
                            }
                        }
                        None => return value::encode_undefined(),
                    };
                    let (buf_handle, dv_offset, dv_length, is_shared) = {
                        let dv_table = caller
                            .data()
                            .dataview_table
                            .lock()
                            .expect("dataview_table mutex");
                        if dv_handle < dv_table.len() {
                            let e = &dv_table[dv_handle];
                            (e.buffer_handle, e.byte_offset, e.byte_length, e.is_shared)
                        } else {
                            return value::encode_undefined();
                        }
                    };
                    let abs_offset = dv_offset as usize + offset as usize;
                    if offset + $size as u32 > dv_length {
                        *caller.data().runtime_error.lock().expect("error mutex") = Some(
                            "RangeError: Offset is outside the bounds of the DataView".to_string(),
                        );
                        return value::encode_undefined();
                    }
                    let bytes = $write(value_arg);
                    if !crate::shared_buffer::dataview_set_bytes(
                        &mut caller,
                        buf_handle,
                        is_shared,
                        abs_offset,
                        &bytes[..$size],
                    ) {
                        *caller.data().runtime_error.lock().expect("error mutex") = Some(
                            "RangeError: Offset is outside the bounds of the DataView".to_string(),
                        );
                        return value::encode_undefined();
                    }
                    value::encode_undefined()
                },
            );
            linker.define(&mut store, "env", $import, $name)?;
        };
    }

    dataview_set_fn!(
        dataview_proto_set_int8_fn,
        "dataview_proto_set_int8",
        1,
        |v: i64| (value::decode_f64(v) as i8).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_uint8_fn,
        "dataview_proto_set_uint8",
        1,
        |v: i64| (value::decode_f64(v) as u8).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_int16_fn,
        "dataview_proto_set_int16",
        2,
        |v: i64| (value::decode_f64(v) as i16).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_uint16_fn,
        "dataview_proto_set_uint16",
        2,
        |v: i64| (value::decode_f64(v) as u16).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_int32_fn,
        "dataview_proto_set_int32",
        4,
        |v: i64| (value::decode_f64(v) as i32).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_uint32_fn,
        "dataview_proto_set_uint32",
        4,
        |v: i64| (value::decode_f64(v) as u32).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_float32_fn,
        "dataview_proto_set_float32",
        4,
        |v: i64| (value::decode_f64(v) as f32).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_float64_fn,
        "dataview_proto_set_float64",
        8,
        |v: i64| value::decode_f64(v).to_le_bytes().to_vec()
    );

    macro_rules! typedarray_constructor {
        ($name:ident, $import:literal, $size:expr, $kind:expr) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>,
                 buffer: i64,
                 byte_offset: i64,
                 length: i64|
                 -> i64 {
                    typedarray_construct(
                        &mut caller,
                        buffer,
                        byte_offset,
                        length,
                        $size,
                        $kind,
                        None,
                    )
                },
            );
            linker.define(&mut store, "env", $import, $name)?;
        };
    }

    typedarray_constructor!(int8array_constructor_fn, "int8array_constructor", 1, 0);
    typedarray_constructor!(uint8array_constructor_fn, "uint8array_constructor", 1, 1);
    typedarray_constructor!(
        uint8clampedarray_constructor_fn,
        "uint8clampedarray_constructor",
        1,
        2
    );
    typedarray_constructor!(int16array_constructor_fn, "int16array_constructor", 2, 0);
    typedarray_constructor!(uint16array_constructor_fn, "uint16array_constructor", 2, 1);
    typedarray_constructor!(int32array_constructor_fn, "int32array_constructor", 4, 0);
    typedarray_constructor!(uint32array_constructor_fn, "uint32array_constructor", 4, 1);
    typedarray_constructor!(
        float32array_constructor_fn,
        "float32array_constructor",
        4,
        3
    );
    typedarray_constructor!(
        float64array_constructor_fn,
        "float64array_constructor",
        8,
        3
    );
    typedarray_constructor!(
        bigint64array_constructor_fn,
        "bigint64array_constructor",
        8,
        4
    );
    typedarray_constructor!(
        biguint64array_constructor_fn,
        "biguint64array_constructor",
        8,
        5
    );
    let typedarray_proto_length_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => match read_object_property_by_name(&mut caller, ptr, "length") {
                    Some(v) => v,
                    None => value::encode_f64(0.0),
                },
                None => value::encode_f64(0.0),
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_length",
        typedarray_proto_length_fn,
    )?;

    let typedarray_proto_byte_length_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => match read_object_property_by_name(&mut caller, ptr, "byteLength") {
                    Some(v) => v,
                    None => value::encode_f64(0.0),
                },
                None => value::encode_f64(0.0),
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_byte_length",
        typedarray_proto_byte_length_fn,
    )?;

    let typedarray_proto_byte_offset_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => match read_object_property_by_name(&mut caller, ptr, "byteOffset") {
                    Some(v) => v,
                    None => value::encode_f64(0.0),
                },
                None => value::encode_f64(0.0),
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_byte_offset",
        typedarray_proto_byte_offset_fn,
    )?;

    let typedarray_proto_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         source: i64,
         offset_val: i64|
         -> i64 {
            let Some(target_entry) = typedarray_entry_from_value(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let offset = if value::is_undefined(offset_val) {
                0u32
            } else {
                value::decode_f64(offset_val) as u32
            };
            if offset > target_entry.length {
                return value::encode_undefined();
            }

            // 先收集源值，保证同一底层缓冲区重叠复制时不会边读边覆盖。
            let values: Vec<i64> = if value::is_array(source) {
                let Some(arr_ptr) = resolve_array_ptr(&mut caller, source) else {
                    return value::encode_undefined();
                };
                let src_length = read_array_length(&mut caller, arr_ptr).unwrap_or(0);
                if offset + src_length > target_entry.length {
                    return value::encode_undefined();
                }
                let mut values = Vec::with_capacity(src_length as usize);
                for i in 0..src_length {
                    values.push(
                        read_array_elem(&mut caller, arr_ptr, i)
                            .unwrap_or_else(value::encode_undefined),
                    );
                }
                values
            } else if let Some(src_entry) = typedarray_entry_from_value(&mut caller, source) {
                if offset + src_entry.length > target_entry.length {
                    return value::encode_undefined();
                }
                let mut values = Vec::with_capacity(src_entry.length as usize);
                for i in 0..src_entry.length {
                    values.push(
                        typedarray_element_read(&mut caller, source, i)
                            .unwrap_or_else(value::encode_undefined),
                    );
                }
                values
            } else {
                return value::encode_undefined();
            };

            for (i, value) in values.into_iter().enumerate() {
                let _ = typedarray_element_write(&mut caller, this_val, offset + i as u32, value);
            }
            value::encode_undefined()
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_set",
        typedarray_proto_set_fn,
    )?;

    let typedarray_proto_slice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         begin_val: i64,
         end_val: i64|
         -> i64 {
            // Resolve TypedArray
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let Some(ptr) =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize)
            else {
                return value::encode_undefined();
            };
            let Some(h) = read_object_property_by_name(&mut caller, ptr, "__typedarray_handle__")
            else {
                return value::encode_undefined();
            };
            let handle = value::decode_f64(h) as usize;
            let ta_table = caller
                .data()
                .typedarray_table
                .lock()
                .expect("typedarray_table mutex");
            let Some(entry) = ta_table.get(handle) else {
                return value::encode_undefined();
            };
            let buf_handle = entry.buffer_handle;
            let byte_offset = entry.byte_offset;
            let length = entry.length;
            let elem_size = entry.element_size;
            let element_kind = entry.element_kind;
            let is_shared = entry.is_shared;
            drop(ta_table);

            // Clamp begin
            let begin = if value::is_undefined(begin_val) {
                0u32
            } else {
                let f = value::decode_f64(begin_val);
                if f < 0.0 {
                    (length as i32 + f as i32).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            // Clamp end
            let end = if value::is_undefined(end_val) {
                length
            } else {
                let f = value::decode_f64(end_val);
                if f < 0.0 {
                    (length as i32 + f as i32).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            let slice_len = end.saturating_sub(begin);
            if slice_len == 0 {
                // Create empty TypedArray
                let new_buf_handle;
                {
                    let mut ab_table = caller
                        .data()
                        .arraybuffer_table
                        .lock()
                        .expect("arraybuffer_table mutex");
                    new_buf_handle = ab_table.len() as u32;
                    ab_table.push(ArrayBufferEntry { data: Vec::new() });
                }
                let new_ta_handle;
                {
                    let mut ta_table = caller
                        .data()
                        .typedarray_table
                        .lock()
                        .expect("typedarray_table mutex");
                    new_ta_handle = ta_table.len() as u32;
                    ta_table.push(TypedArrayEntry {
                        buffer_handle: new_buf_handle,
                        byte_offset: 0,
                        length: 0,
                        element_size: elem_size,
                        element_kind,
                        is_shared: false,
                    });
                }
                let obj = {
                    let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                    alloc_host_object(&mut caller, &_wjsm_env, 4)
                };
                let _ = define_host_data_property_from_caller(
                    &mut caller,
                    obj,
                    "__typedarray_handle__",
                    value::encode_f64(new_ta_handle as f64),
                );
                let _ = define_host_data_property_from_caller(
                    &mut caller,
                    obj,
                    "length",
                    value::encode_f64(0.0),
                );
                let _ = define_host_data_property_from_caller(
                    &mut caller,
                    obj,
                    "byteLength",
                    value::encode_f64(0.0),
                );
                let _ = define_host_data_property_from_caller(
                    &mut caller,
                    obj,
                    "byteOffset",
                    value::encode_f64(0.0),
                );
                return obj;
            }

            // Create new ArrayBuffer with sliced bytes
            let src_byte_start = byte_offset as usize + (begin as usize) * (elem_size as usize);
            let slice_byte_len = slice_len as usize * elem_size as usize;
            let sliced_data: Vec<u8> = if is_shared {
                let shared = caller
                    .data()
                    .shared_state
                    .clone()
                    .expect("SharedArrayBuffer requires shared_state");
                let sab_table = shared.sab_table.lock().expect("sab_table mutex");
                let Some(buf_entry) = sab_table.get(buf_handle as usize) else {
                    return value::encode_undefined();
                };
                let guard = buf_entry.data.read().expect("sab read lock");
                let end_off = src_byte_start + slice_byte_len;
                if end_off > guard.len() {
                    return value::encode_undefined();
                }
                let data = guard[src_byte_start..end_off].to_vec();
                drop(guard);
                drop(sab_table);
                data
            } else {
                let ab_table = caller
                    .data()
                    .arraybuffer_table
                    .lock()
                    .expect("arraybuffer_table mutex");
                let Some(buf_entry) = ab_table.get(buf_handle as usize) else {
                    return value::encode_undefined();
                };
                let end_off = src_byte_start + slice_byte_len;
                if end_off > buf_entry.data.len() {
                    return value::encode_undefined();
                }
                let data = buf_entry.data[src_byte_start..end_off].to_vec();
                drop(ab_table);
                data
            };
            let new_buf_handle;
            {
                let mut ab_table = caller
                    .data()
                    .arraybuffer_table
                    .lock()
                    .expect("arraybuffer_table mutex");
                new_buf_handle = ab_table.len() as u32;
                ab_table.push(ArrayBufferEntry { data: sliced_data });
            }

            // Create new TypedArray entry pointing to the new buffer
            let new_ta_handle;
            {
                let mut ta_table = caller
                    .data()
                    .typedarray_table
                    .lock()
                    .expect("typedarray_table mutex");
                new_ta_handle = ta_table.len() as u32;
                ta_table.push(TypedArrayEntry {
                    buffer_handle: new_buf_handle,
                    byte_offset: 0,
                    length: slice_len,
                    element_size: elem_size,
                    element_kind,
                    is_shared: false,
                });
            }

            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 4)
            };
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__typedarray_handle__",
                value::encode_f64(new_ta_handle as f64),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "length",
                value::encode_f64(slice_len as f64),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "byteLength",
                value::encode_f64((slice_len * elem_size as u32) as f64),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "byteOffset",
                value::encode_f64(0.0),
            );
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_slice",
        typedarray_proto_slice_fn,
    )?;

    let typedarray_proto_subarray_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         begin_val: i64,
         end_val: i64|
         -> i64 {
            // Resolve TypedArray
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let Some(ptr) =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize)
            else {
                return value::encode_undefined();
            };
            let Some(h) = read_object_property_by_name(&mut caller, ptr, "__typedarray_handle__")
            else {
                return value::encode_undefined();
            };
            let handle = value::decode_f64(h) as usize;
            let ta_table = caller
                .data()
                .typedarray_table
                .lock()
                .expect("typedarray_table mutex");
            let Some(entry) = ta_table.get(handle) else {
                return value::encode_undefined();
            };
            let buf_handle = entry.buffer_handle;
            let byte_offset = entry.byte_offset;
            let length = entry.length;
            let elem_size = entry.element_size;
            let element_kind = entry.element_kind;
            let sub_is_shared = entry.is_shared;
            drop(ta_table);

            // Clamp begin
            let begin = if value::is_undefined(begin_val) {
                0u32
            } else {
                let f = value::decode_f64(begin_val);
                if f < 0.0 {
                    (length as i32 + f as i32).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            // Clamp end
            let end = if value::is_undefined(end_val) {
                length
            } else {
                let f = value::decode_f64(end_val);
                if f < 0.0 {
                    (length as i32 + f as i32).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            let sub_len = end.saturating_sub(begin);
            let new_byte_offset = byte_offset + begin * elem_size as u32;

            // Create new TypedArray entry sharing the same ArrayBuffer
            let new_ta_handle;
            {
                let mut ta_table = caller
                    .data()
                    .typedarray_table
                    .lock()
                    .expect("typedarray_table mutex");
                new_ta_handle = ta_table.len() as u32;
                ta_table.push(TypedArrayEntry {
                    buffer_handle: buf_handle,
                    byte_offset: new_byte_offset,
                    length: sub_len,
                    element_size: elem_size,
                    element_kind,
                    is_shared: sub_is_shared,
                });
            }

            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 4)
            };
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__typedarray_handle__",
                value::encode_f64(new_ta_handle as f64),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "length",
                value::encode_f64(sub_len as f64),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "byteLength",
                value::encode_f64((sub_len * elem_size as u32) as f64),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "byteOffset",
                value::encode_f64(new_byte_offset as f64),
            );
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_subarray",
        typedarray_proto_subarray_fn,
    )?;

    let create_global_object_fn =
        Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>| -> i64 {
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 64)
            };
            let builtin_pairs: &[(&str, NativeCallable)] = &[
                ("Array", NativeCallable::ArrayConstructor),
                ("Object", NativeCallable::ObjectConstructor),
                ("Function", NativeCallable::FunctionConstructor),
                ("String", NativeCallable::StringConstructor),
                ("Boolean", NativeCallable::BooleanConstructor),
                ("Number", NativeCallable::NumberConstructor),
                ("Symbol", NativeCallable::SymbolConstructor),
                ("BigInt", NativeCallable::BigIntConstructor),
                ("RegExp", NativeCallable::RegExpConstructor),
                ("Error", NativeCallable::ErrorConstructor),
                ("TypeError", NativeCallable::TypeErrorConstructor),
                ("RangeError", NativeCallable::RangeErrorConstructor),
                ("SyntaxError", NativeCallable::SyntaxErrorConstructor),
                ("ReferenceError", NativeCallable::ReferenceErrorConstructor),
                ("URIError", NativeCallable::URIErrorConstructor),
                ("EvalError", NativeCallable::EvalErrorConstructor),
                ("AggregateError", NativeCallable::AggregateErrorConstructor),
                ("Map", NativeCallable::MapConstructor),
                ("Set", NativeCallable::SetConstructor),
                ("WeakMap", NativeCallable::WeakMapConstructor),
                ("WeakSet", NativeCallable::WeakSetConstructor),
                ("WeakRef", NativeCallable::WeakRefConstructor),
                (
                    "FinalizationRegistry",
                    NativeCallable::FinalizationRegistryConstructor,
                ),
                ("Date", NativeCallable::DateConstructorGlobal),
                ("Promise", NativeCallable::PromiseConstructor),
                ("Headers", NativeCallable::HeadersConstructor),
                ("Request", NativeCallable::RequestConstructor),
                ("Response", NativeCallable::ResponseConstructor),
                ("ReadableStream", NativeCallable::ReadableStreamConstructor),
                ("WritableStream", NativeCallable::WritableStreamConstructor),
                (
                    "TransformStream",
                    NativeCallable::TransformStreamConstructor,
                ),
                (
                    "CountQueuingStrategy",
                    NativeCallable::CountQueuingStrategyConstructor,
                ),
                (
                    "ByteLengthQueuingStrategy",
                    NativeCallable::ByteLengthQueuingStrategyConstructor,
                ),
                (
                    "AbortController",
                    NativeCallable::AbortControllerConstructor,
                ),
                ("ArrayBuffer", NativeCallable::ArrayBufferConstructorGlobal),
                (
                    "SharedArrayBuffer",
                    NativeCallable::SharedArrayBufferConstructor,
                ),
                ("Atomics", NativeCallable::AtomicsGlobal),
                ("DataView", NativeCallable::DataViewConstructorGlobal),
                ("BigInt64Array", NativeCallable::BigInt64ArrayConstructor),
                ("BigUint64Array", NativeCallable::BigUint64ArrayConstructor),
                ("Proxy", NativeCallable::ProxyConstructor),
                ("gc", NativeCallable::GcCollect),
            ];

            for (name, callable) in builtin_pairs {
                let mut native_callables = caller.data().native_callables.lock().unwrap();
                let idx = native_callables.len() as u32;
                native_callables.push(callable.clone());
                let val = value::encode_native_callable_idx(idx);
                drop(native_callables);
                let _ = define_host_data_property_from_caller(&mut caller, obj, name, val);
            }

            let _ = define_host_data_property_from_caller(&mut caller, obj, "globalThis", obj);

            // test262 harness: global `$262` with `.agent` methods
            let agent_obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 7)
            };
            let agent_methods: &[(&str, NativeCallable)] = &[
                ("start", NativeCallable::AgentStart),
                ("broadcast", NativeCallable::AgentBroadcast),
                ("receiveBroadcast", NativeCallable::AgentReceiveBroadcast),
                ("getReport", NativeCallable::AgentGetReport),
                ("report", NativeCallable::AgentReport),
                ("sleep", NativeCallable::AgentSleep),
                ("monotonicNow", NativeCallable::AgentMonotonicNow),
            ];
            for (name, callable) in agent_methods {
                let mut nc = caller.data().native_callables.lock().unwrap();
                let idx = nc.len() as u32;
                nc.push(callable.clone());
                let val = value::encode_native_callable_idx(idx);
                drop(nc);
                let _ = define_host_data_property_from_caller(&mut caller, agent_obj, name, val);
            }
            let harness_obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 1)
            };
            let _ =
                define_host_data_property_from_caller(&mut caller, harness_obj, "agent", agent_obj);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "$262", harness_obj);

            obj
        });
    linker.define(
        &mut store,
        "env",
        "create_global_object",
        create_global_object_fn,
    )?;

    let create_exception_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, thrown_value: i64| -> i64 {
            let rendered =
                render_value(&mut caller, thrown_value).unwrap_or_else(|_| "unknown".to_string());
            let mut errors = caller.data().error_table.lock().unwrap();
            let idx = errors.len() as u32;
            errors.push(ErrorEntry {
                name: String::new(),
                message: rendered,
                value: thrown_value,
            });
            value::encode_handle(value::TAG_EXCEPTION, idx)
        },
    );
    linker.define(&mut store, "env", "create_exception", create_exception_fn)?;

    let exception_value_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, exception_handle: i64| -> i64 {
            let idx = value::decode_handle(exception_handle) as usize;
            let errors = caller.data().error_table.lock().unwrap();
            errors
                .get(idx)
                .map(|e| e.value)
                .unwrap_or(value::encode_undefined())
        },
    );
    linker.define(&mut store, "env", "exception_value", exception_value_fn)?;

    let date_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         _this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let args: Vec<i64> = if args_count > 0 {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_undefined();
                };
                let data = memory.data(&caller);
                let base = args_base as usize;
                let mut result = Vec::with_capacity(args_count as usize);
                for i in 0..args_count as usize {
                    let offset = base + i * 8;
                    if offset + 8 <= data.len() {
                        let mut bytes = [0u8; 8];
                        bytes.copy_from_slice(&data[offset..offset + 8]);
                        result.push(i64::from_le_bytes(bytes));
                    } else {
                        result.push(value::encode_undefined());
                    }
                }
                result
            } else {
                vec![]
            };

            let ms = if args.is_empty() {
                let now = chrono::Utc::now();
                now.timestamp_millis() as f64
            } else if args.len() == 1 {
                let arg = args[0];
                if value::is_undefined(arg) {
                    let now = chrono::Utc::now();
                    now.timestamp_millis() as f64
                } else if value::is_f64(arg) {
                    let val = value::decode_f64(arg);
                    if val.is_nan() || val.is_infinite() {
                        f64::NAN
                    } else {
                        val
                    }
                } else if value::is_string(arg) {
                    let s = read_value_string_bytes(&mut caller, arg)
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default();
                    if s.is_empty() {
                        f64::NAN
                    } else {
                        match DateTime::parse_from_rfc3339(&s) {
                            Ok(dt) => dt.timestamp_millis() as f64,
                            Err(_) => {
                                match chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S")
                                {
                                    Ok(ndt) => ndt.and_utc().timestamp_millis() as f64,
                                    Err(_) => {
                                        match chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
                                            Ok(nd) => nd
                                                .and_hms_opt(0, 0, 0)
                                                .unwrap()
                                                .and_utc()
                                                .timestamp_millis()
                                                as f64,
                                            Err(_) => f64::NAN,
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    f64::NAN
                }
            } else {
                date_args_to_ms(&args, false)
            };

            let state = caller.data();
            let (
                get_date_fn,
                get_day_fn,
                get_full_year_fn,
                get_hours_fn,
                get_milliseconds_fn,
                get_minutes_fn,
                get_month_fn,
                get_seconds_fn,
                get_time_fn,
                get_timezone_offset_fn,
                get_utc_date_fn,
                get_utc_day_fn,
                get_utc_full_year_fn,
                get_utc_hours_fn,
                get_utc_milliseconds_fn,
                get_utc_minutes_fn,
                get_utc_month_fn,
                get_utc_seconds_fn,
                set_date_fn,
                set_full_year_fn,
                set_hours_fn,
                set_milliseconds_fn,
                set_minutes_fn,
                set_month_fn,
                set_seconds_fn,
                set_time_fn,
                set_utc_date_fn,
                set_utc_full_year_fn,
                set_utc_hours_fn,
                set_utc_milliseconds_fn,
                set_utc_minutes_fn,
                set_utc_month_fn,
                set_utc_seconds_fn,
                to_string_fn,
                to_date_string_fn,
                to_time_string_fn,
                to_iso_string_fn,
                to_utc_string_fn,
                to_json_fn,
                value_of_fn,
            ) = {
                (
                    create_date_method(state, DateMethodKind::GetDate),
                    create_date_method(state, DateMethodKind::GetDay),
                    create_date_method(state, DateMethodKind::GetFullYear),
                    create_date_method(state, DateMethodKind::GetHours),
                    create_date_method(state, DateMethodKind::GetMilliseconds),
                    create_date_method(state, DateMethodKind::GetMinutes),
                    create_date_method(state, DateMethodKind::GetMonth),
                    create_date_method(state, DateMethodKind::GetSeconds),
                    create_date_method(state, DateMethodKind::GetTime),
                    create_date_method(state, DateMethodKind::GetTimezoneOffset),
                    create_date_method(state, DateMethodKind::GetUTCDate),
                    create_date_method(state, DateMethodKind::GetUTCDay),
                    create_date_method(state, DateMethodKind::GetUTCFullYear),
                    create_date_method(state, DateMethodKind::GetUTCHours),
                    create_date_method(state, DateMethodKind::GetUTCMilliseconds),
                    create_date_method(state, DateMethodKind::GetUTCMinutes),
                    create_date_method(state, DateMethodKind::GetUTCMonth),
                    create_date_method(state, DateMethodKind::GetUTCSeconds),
                    create_date_method(state, DateMethodKind::SetDate),
                    create_date_method(state, DateMethodKind::SetFullYear),
                    create_date_method(state, DateMethodKind::SetHours),
                    create_date_method(state, DateMethodKind::SetMilliseconds),
                    create_date_method(state, DateMethodKind::SetMinutes),
                    create_date_method(state, DateMethodKind::SetMonth),
                    create_date_method(state, DateMethodKind::SetSeconds),
                    create_date_method(state, DateMethodKind::SetTime),
                    create_date_method(state, DateMethodKind::SetUTCDate),
                    create_date_method(state, DateMethodKind::SetUTCFullYear),
                    create_date_method(state, DateMethodKind::SetUTCHours),
                    create_date_method(state, DateMethodKind::SetUTCMilliseconds),
                    create_date_method(state, DateMethodKind::SetUTCMinutes),
                    create_date_method(state, DateMethodKind::SetUTCMonth),
                    create_date_method(state, DateMethodKind::SetUTCSeconds),
                    create_date_method(state, DateMethodKind::ToString),
                    create_date_method(state, DateMethodKind::ToDateString),
                    create_date_method(state, DateMethodKind::ToTimeString),
                    create_date_method(state, DateMethodKind::ToISOString),
                    create_date_method(state, DateMethodKind::ToUTCString),
                    create_date_method(state, DateMethodKind::ToJSON),
                    create_date_method(state, DateMethodKind::ValueOf),
                )
            };

            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 40)
            };
            let ms_val = value::encode_f64(ms);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__date_ms__", ms_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getDate", get_date_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getDay", get_day_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getFullYear",
                get_full_year_fn,
            );
            let _ =
                define_host_data_property_from_caller(&mut caller, obj, "getHours", get_hours_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getMilliseconds",
                get_milliseconds_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getMinutes",
                get_minutes_fn,
            );
            let _ =
                define_host_data_property_from_caller(&mut caller, obj, "getMonth", get_month_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getSeconds",
                get_seconds_fn,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getTime", get_time_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getTimezoneOffset",
                get_timezone_offset_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCDate",
                get_utc_date_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCDay",
                get_utc_day_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCFullYear",
                get_utc_full_year_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCHours",
                get_utc_hours_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCMilliseconds",
                get_utc_milliseconds_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCMinutes",
                get_utc_minutes_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCMonth",
                get_utc_month_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCSeconds",
                get_utc_seconds_fn,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setDate", set_date_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setFullYear",
                set_full_year_fn,
            );
            let _ =
                define_host_data_property_from_caller(&mut caller, obj, "setHours", set_hours_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setMilliseconds",
                set_milliseconds_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setMinutes",
                set_minutes_fn,
            );
            let _ =
                define_host_data_property_from_caller(&mut caller, obj, "setMonth", set_month_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setSeconds",
                set_seconds_fn,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setTime", set_time_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCDate",
                set_utc_date_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCFullYear",
                set_utc_full_year_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCHours",
                set_utc_hours_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCMilliseconds",
                set_utc_milliseconds_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCMinutes",
                set_utc_minutes_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCMonth",
                set_utc_month_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCSeconds",
                set_utc_seconds_fn,
            );
            let _ =
                define_host_data_property_from_caller(&mut caller, obj, "toString", to_string_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "toDateString",
                to_date_string_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "toTimeString",
                to_time_string_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "toISOString",
                to_iso_string_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "toUTCString",
                to_utc_string_fn,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toJSON", to_json_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "valueOf", value_of_fn);
            obj
        },
    );
    linker.define(&mut store, "env", "date_constructor", date_constructor_fn)?;

    let date_now_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>| -> i64 {
        let now = chrono::Utc::now();
        value::encode_f64(now.timestamp_millis() as f64)
    });
    linker.define(&mut store, "env", "date_now", date_now_fn)?;

    let date_parse_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let s = if value::is_string(arg) {
                read_value_string_bytes(&mut caller, arg)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            if s.is_empty() {
                return value::encode_f64(f64::NAN);
            }
            match DateTime::parse_from_rfc3339(&s) {
                Ok(dt) => value::encode_f64(dt.timestamp_millis() as f64),
                Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S") {
                    Ok(ndt) => value::encode_f64(ndt.and_utc().timestamp_millis() as f64),
                    Err(_) => {
                        match chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S%.f") {
                            Ok(ndt) => value::encode_f64(ndt.and_utc().timestamp_millis() as f64),
                            Err(_) => match chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
                                Ok(nd) => value::encode_f64(
                                    nd.and_hms_opt(0, 0, 0)
                                        .unwrap()
                                        .and_utc()
                                        .timestamp_millis()
                                        as f64,
                                ),
                                Err(_) => {
                                    match chrono::NaiveDateTime::parse_from_str(&s, "%b %d, %Y") {
                                        Ok(ndt) => value::encode_f64(
                                            ndt.and_utc().timestamp_millis() as f64,
                                        ),
                                        Err(_) => match chrono::NaiveDateTime::parse_from_str(
                                            &s,
                                            "%B %d, %Y",
                                        ) {
                                            Ok(ndt) => value::encode_f64(
                                                ndt.and_utc().timestamp_millis() as f64,
                                            ),
                                            Err(_) => match chrono::NaiveDateTime::parse_from_str(
                                                &s,
                                                "%d %b %Y %H:%M:%S",
                                            ) {
                                                Ok(ndt) => value::encode_f64(
                                                    ndt.and_utc().timestamp_millis() as f64,
                                                ),
                                                Err(_) => value::encode_f64(f64::NAN),
                                            },
                                        },
                                    }
                                }
                            },
                        }
                    }
                },
            }
        },
    );
    linker.define(&mut store, "env", "date_parse", date_parse_fn)?;

    let date_utc_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let args = vec![arg];
            let ms = date_args_to_ms(&args, true);
            value::encode_f64(ms)
        },
    );
    linker.define(&mut store, "env", "date_utc", date_utc_fn)?;

    // TODO: 当前私有字段实现仅通过 "#fieldName" 字符串作为属性键存储在对象的普通属性槽中，
    // 不符合 ECMAScript 规范的 [[PrivateElements]] 语义。任何代码都可以通过 obj["#x"] 访问，
    // 且没有基于类身份的访问控制。未来需要重构为基于类身份的私有槽机制。
    let private_get_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) {
                return make_type_error_exception(
                    &mut caller,
                    "TypeError: Cannot read private member from a non-object",
                );
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return make_type_error_exception(
                    &mut caller,
                    "TypeError: Cannot read private member from a non-object",
                );
            };
            // 品牌检查（PrivateElementFind）：实例上不存在该私有槽 → 同步抛出可捕获
            // TypeError，而非延迟报错/返回 undefined。私有字段以 "#name" 受控属性键存储，
            // 槽缺失即表示该对象不是声明此私有名的类的实例。
            match read_object_property_by_name_id(&mut caller, ptr, key_name_id as u32) {
                Some(val) => val,
                None => make_type_error_exception(
                    &mut caller,
                    "TypeError: Cannot read private member from an object whose class did not declare it",
                ),
            }
        },
    );
    linker.define(&mut store, "env", "private_get", private_get_fn)?;

    let private_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32, val: i64| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: cannot write private member to non-object".to_string());
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let found_slot = find_property_slot_by_name_id(&mut caller, ptr, key_name_id as u32);
            if let Some((slot_offset, _flags, _old_val)) = found_slot {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_undefined();
                };
                let data = memory.data_mut(&mut caller);
                data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
                val
            } else {
                write_object_property_by_name_id(&mut caller, ptr, obj, key_name_id as u32, val, 0);
                val
            }
        },
    );
    linker.define(&mut store, "env", "private_set", private_set_fn)?;

    let private_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) {
                return value::encode_bool(false);
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_bool(false);
            };
            let found = find_property_slot_by_name_id(&mut caller, ptr, key_name_id as u32);
            value::encode_bool(found.is_some())
        },
    );
    linker.define(&mut store, "env", "private_has", private_has_fn)?;

    Ok(())
}
