//! Collection (Map/Set/WeakMap/WeakSet) method dispatch.
//!
//! Extracted from runtime_builtins.rs to concentrate all collection-related
//! logic (Map/Set operations, WeakMap/WeakSet operations).

use super::*;

pub(crate) fn is_object_key(key: i64) -> bool {
    value::is_object(key) || value::is_array(key) || value::is_function(key)
}

/// 为 Map/Set 创建 keys / values / entries 迭代器（与 NativeCallable 路径共用）。
pub(crate) fn map_set_create_iterator(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: MapSetMethodKind,
) -> i64 {
    if !value::is_object(this_val) {
        return value::encode_undefined();
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let Some(op) = obj_ptr else {
        return value::encode_undefined();
    };
    let map_handle = read_object_property_by_name(caller, op, "__map_handle__");
    let set_handle = read_object_property_by_name(caller, op, "__set_handle__");
    match kind {
        MapSetMethodKind::Keys => {
            if let Some(mh) = map_handle {
                let map_handle_u32 = value::decode_f64(mh) as u32;
                let table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
                if (map_handle_u32 as usize) < table.len() {
                    drop(table);
                    let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::MapKeyIter {
                        map_handle: map_handle_u32,
                        index: 0,
                    });
                    return value::encode_handle(value::TAG_ITERATOR, iter_handle);
                }
            }
            if let Some(sh) = set_handle {
                let set_handle_u32 = value::decode_f64(sh) as u32;
                let table = caller.data().set_table.lock().unwrap_or_else(|e| e.into_inner());
                if (set_handle_u32 as usize) < table.len() {
                    drop(table);
                    let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::SetValueIter {
                        set_handle: set_handle_u32,
                        index: 0,
                    });
                    return value::encode_handle(value::TAG_ITERATOR, iter_handle);
                }
            }
            value::encode_undefined()
        }
        MapSetMethodKind::Values => {
            if let Some(mh) = map_handle {
                let map_handle_u32 = value::decode_f64(mh) as u32;
                let table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
                if (map_handle_u32 as usize) < table.len() {
                    drop(table);
                    let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::MapValueIter {
                        map_handle: map_handle_u32,
                        index: 0,
                    });
                    return value::encode_handle(value::TAG_ITERATOR, iter_handle);
                }
            }
            if let Some(sh) = set_handle {
                let set_handle_u32 = value::decode_f64(sh) as u32;
                let table = caller.data().set_table.lock().unwrap_or_else(|e| e.into_inner());
                if (set_handle_u32 as usize) < table.len() {
                    drop(table);
                    let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::SetValueIter {
                        set_handle: set_handle_u32,
                        index: 0,
                    });
                    return value::encode_handle(value::TAG_ITERATOR, iter_handle);
                }
            }
            value::encode_undefined()
        }
        MapSetMethodKind::Entries => {
            if let Some(mh) = map_handle {
                let map_handle_u32 = value::decode_f64(mh) as u32;
                let table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
                if (map_handle_u32 as usize) < table.len() {
                    drop(table);
                    let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::MapEntryIter {
                        map_handle: map_handle_u32,
                        index: 0,
                    });
                    return value::encode_handle(value::TAG_ITERATOR, iter_handle);
                }
            }
            if let Some(sh) = set_handle {
                let set_handle_u32 = value::decode_f64(sh) as u32;
                let table = caller.data().set_table.lock().unwrap_or_else(|e| e.into_inner());
                if (set_handle_u32 as usize) < table.len() {
                    drop(table);
                    let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::SetValueIter {
                        set_handle: set_handle_u32,
                        index: 0,
                    });
                    return value::encode_handle(value::TAG_ITERATOR, iter_handle);
                }
            }
            value::encode_undefined()
        }
        _ => value::encode_undefined(),
    }
}

/// Map/Set.prototype.forEach：遍历并调用 callback（同步宿主 import 路径）。
pub(crate) fn map_set_for_each_impl(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: &[i64],
) -> i64 {
    let Some(cb) = args.first().copied() else {
        return value::encode_undefined();
    };
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if !value::is_object(this_val) {
        return value::encode_undefined();
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let Some(op) = obj_ptr else {
        return value::encode_undefined();
    };
    let map_handle = read_object_property_by_name(caller, op, "__map_handle__");
    let set_handle = read_object_property_by_name(caller, op, "__set_handle__");
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let rt = tokio::runtime::Handle::current();
    if let Some(mh) = map_handle {
        let handle = value::decode_f64(mh) as usize;
        let pairs: Vec<(i64, i64)> = {
            let table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
            if handle >= table.len() {
                return value::encode_undefined();
            }
            let entry = &table[handle];
            entry
                .keys
                .iter()
                .zip(entry.values.iter())
                .map(|(&k, &v)| (k, v))
                .collect()
        };
        for (key, val) in pairs {
            if rt
                .block_on(invoke_resolved_callback_async_option(
                    caller, &env, cb, this_arg, &[val, key, this_val],
                ))
                .is_none()
            {
                return value::encode_undefined();
            }
        }
        return value::encode_undefined();
    }
    if let Some(sh) = set_handle {
        let handle = value::decode_f64(sh) as usize;
        let values: Vec<i64> = {
            let table = caller.data().set_table.lock().unwrap_or_else(|e| e.into_inner());
            if handle >= table.len() {
                return value::encode_undefined();
            }
            table[handle].values.clone()
        };
        for val in values {
            if rt
                .block_on(invoke_resolved_callback_async_option(
                    caller, &env, cb, this_arg, &[val, val, this_val],
                ))
                .is_none()
            {
                return value::encode_undefined();
            }
        }
    }
    value::encode_undefined()
}

pub(crate) fn call_weakmap_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: WeakMapMethodKind,
    args: Vec<i64>,
) -> i64 {
    match kind {
        WeakMapMethodKind::Set => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let val = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                *caller.data().runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some("TypeError: Invalid value used as weak map key".to_string());
                return this_val;
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller
                    .data()
                    .weakmap_table.lock().unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    table[handle].map.insert(key_handle, val);
                }
            }
            this_val
        }
        WeakMapMethodKind::Get => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_undefined();
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller
                .data()
                .weakmap_table.lock().unwrap_or_else(|e| e.into_inner());
            if handle < table.len()
                && let Some(&val) = table[handle].map.get(&key_handle)
            {
                return val;
            }
            value::encode_undefined()
        }
        WeakMapMethodKind::Has => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller
                .data()
                .weakmap_table.lock().unwrap_or_else(|e| e.into_inner());
            if handle < table.len() {
                return value::encode_bool(table[handle].map.contains_key(&key_handle));
            }
            value::encode_bool(false)
        }
        WeakMapMethodKind::Delete => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let mut table = caller
                .data()
                .weakmap_table.lock().unwrap_or_else(|e| e.into_inner());
            if handle < table.len() {
                return value::encode_bool(table[handle].map.remove(&key_handle).is_some());
            }
            value::encode_bool(false)
        }
    }
}

pub(crate) fn call_weakset_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: WeakSetMethodKind,
    args: Vec<i64>,
) -> i64 {
    match kind {
        WeakSetMethodKind::Add => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                *caller.data().runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some("TypeError: Invalid value used in weak set".to_string());
                return this_val;
            }
            let handle = read_weakset_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller
                    .data()
                    .weakset_table.lock().unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    table[handle].set.insert(key_handle);
                }
            }
            this_val
        }
        WeakSetMethodKind::Has => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let handle = read_weakset_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller
                .data()
                .weakset_table.lock().unwrap_or_else(|e| e.into_inner());
            if handle < table.len() {
                return value::encode_bool(table[handle].set.contains(&key_handle));
            }
            value::encode_bool(false)
        }
        WeakSetMethodKind::Delete => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let handle = read_weakset_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let mut table = caller
                .data()
                .weakset_table.lock().unwrap_or_else(|e| e.into_inner());
            if handle < table.len() {
                return value::encode_bool(table[handle].set.remove(&key_handle));
            }
            value::encode_bool(false)
        }
    }
}
pub(crate) fn call_map_set_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: MapSetMethodKind,
    args: Vec<i64>,
) -> i64 {
    if !value::is_object(this_val) {
        return value::encode_undefined();
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let Some(op) = obj_ptr else {
        return value::encode_undefined();
    };
    let map_handle = read_object_property_by_name(caller, op, "__map_handle__");
    let set_handle = read_object_property_by_name(caller, op, "__set_handle__");

    match kind {
        MapSetMethodKind::MapSet => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let val = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let mut table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    let entry = &mut table[handle];
                    for i in 0..entry.keys.len() {
                        if same_value_zero(entry.keys[i], key) {
                            entry.values[i] = val;
                            return this_val;
                        }
                    }
                    entry.keys.push(key);
                    entry.values.push(val);
                }
            }
            this_val
        }
        MapSetMethodKind::MapGet => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    let entry = &table[handle];
                    for i in 0..entry.keys.len() {
                        if same_value_zero(entry.keys[i], key) {
                            return entry.values[i];
                        }
                    }
                }
            }
            value::encode_undefined()
        }
        MapSetMethodKind::SetAdd => {
            let val = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let mut table = caller.data().set_table.lock().unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    let entry = &mut table[handle];
                    for i in 0..entry.values.len() {
                        if same_value_zero(entry.values[i], val) {
                            return this_val;
                        }
                    }
                    entry.values.push(val);
                }
            }
            this_val
        }
        MapSetMethodKind::Has => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
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
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let table = caller.data().set_table.lock().unwrap_or_else(|e| e.into_inner());
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
            value::encode_bool(false)
        }
        MapSetMethodKind::Delete => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let mut table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
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
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let mut table = caller.data().set_table.lock().unwrap_or_else(|e| e.into_inner());
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
            value::encode_bool(false)
        }
        MapSetMethodKind::Clear => {
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let mut table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    table[handle].keys.clear();
                    table[handle].values.clear();
                }
                return value::encode_undefined();
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let mut table = caller.data().set_table.lock().unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    table[handle].values.clear();
                }
                return value::encode_undefined();
            }
            value::encode_undefined()
        }
        MapSetMethodKind::Size => {
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    return value::encode_f64(table[handle].keys.len() as f64);
                }
                return value::encode_f64(0.0);
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let table = caller.data().set_table.lock().unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    return value::encode_f64(table[handle].values.len() as f64);
                }
                return value::encode_f64(0.0);
            }
            value::encode_f64(0.0)
        }
        MapSetMethodKind::ForEach => map_set_for_each_impl(caller, this_val, &args),
        MapSetMethodKind::Keys => map_set_create_iterator(caller, this_val, MapSetMethodKind::Keys),
        MapSetMethodKind::Values => {
            map_set_create_iterator(caller, this_val, MapSetMethodKind::Values)
        }
        MapSetMethodKind::Entries => {
            map_set_create_iterator(caller, this_val, MapSetMethodKind::Entries)
        }
    }
}
