//! Collection (Map/Set/WeakMap/WeakSet) method dispatch.
//!
//! Extracted from runtime_builtins.rs to concentrate all collection-related
//! logic (Map/Set operations, WeakMap/WeakSet operations).

use super::*;

pub(crate) fn is_object_key(key: i64) -> bool {
    value::is_object(key)
        || value::is_array(key)
        || value::is_function(key)
        || value::is_symbol(key)
}

fn collection_handles(
    caller: &mut Caller<'_, RuntimeState>,
    receiver: i64,
) -> (Option<i64>, Option<i64>) {
    #[cfg(feature = "managed-heap-v2")]
    {
        (
            read_host_data_property_v2(caller, receiver, "__map_handle__"),
            read_host_data_property_v2(caller, receiver, "__set_handle__"),
        )
    }
    #[cfg(not(feature = "managed-heap-v2"))]
    {
        let Some(object) =
            resolve_handle_idx(caller, value::decode_object_handle(receiver) as usize)
        else {
            return (None, None);
        };
        (
            read_object_property_by_name(caller, object, "__map_handle__"),
            read_object_property_by_name(caller, object, "__set_handle__"),
        )
    }
}

pub(crate) async fn fill_map_from_constructor_arg_async(
    caller: &mut Caller<'_, RuntimeState>,
    handle: u32,
    arg: i64,
) -> bool {
    let Some(values) = collect_constructor_iterable_values_async(caller, arg).await else {
        return false;
    };
    let mut pairs = Vec::with_capacity(values.len());
    for entry_val in values {
        let Some(pair) = map_entry_pair_from_value(caller, entry_val) else {
            return false;
        };
        pairs.push(pair);
    }
    let mut table = caller
        .data()
        .map_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(map_entry) = table.get_mut(handle as usize) {
        for (key, val) in pairs {
            if let Some(pos) = map_entry
                .keys
                .iter()
                .position(|existing| same_value_zero(caller, *existing, key))
            {
                map_entry.values[pos] = val;
            } else {
                map_entry.keys.push(key);
                map_entry.values.push(val);
            }
        }
    }
    true
}

pub(crate) async fn fill_set_from_constructor_arg_async(
    caller: &mut Caller<'_, RuntimeState>,
    handle: u32,
    arg: i64,
) -> bool {
    let Some(values) = collect_constructor_iterable_values_async(caller, arg).await else {
        return false;
    };
    let mut table = caller
        .data()
        .set_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(set_entry) = table.get_mut(handle as usize) {
        for val in values {
            if !set_entry
                .values
                .iter()
                .any(|existing| same_value_zero(caller, *existing, val))
            {
                set_entry.values.push(val);
            }
        }
    }
    true
}

async fn collect_constructor_iterable_values_async(
    caller: &mut Caller<'_, RuntimeState>,
    source: i64,
) -> Option<Vec<i64>> {
    if value::is_undefined(source) || value::is_null(source) {
        return Some(Vec::new());
    }
    if value::is_object(source) || value::is_array(source) || value::is_function(source) {
        match get_method_by_name_id(
            caller,
            source,
            encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR),
        ) {
            Ok(Some(method)) => {
                let iterator = call_iterable_method_async(caller, method, source).await;
                return collect_iterator_object_values_async(caller, iterator).await;
            }
            Ok(None) => {}
            Err(exception) => return Some(vec![exception]),
        }
    }
    let iterator = iterator_from_impl_async(caller, source).await;
    collect_iterator_object_values_async(caller, iterator).await
}

async fn collect_iterator_object_values_async(
    caller: &mut Caller<'_, RuntimeState>,
    iterator: i64,
) -> Option<Vec<i64>> {
    #[cfg(feature = "managed-heap-v2")]
    {
        return collect_iterator_object_values_v2_async(caller, iterator).await;
    }
    #[cfg(not(feature = "managed-heap-v2"))]
    {
        let iterator = if value::is_iterator(iterator) {
            create_raw_iterator_object(caller, iterator)
        } else {
            iterator
        };
        let iter_ptr = resolve_handle(caller, iterator)?;
        let next = read_object_property_by_name(caller, iter_ptr, "next")?;
        if !value::is_callable(next) {
            set_runtime_error(
                caller.data(),
                "TypeError: iterator next is not callable".to_string(),
            );
            return None;
        }
        let mut out = Vec::new();
        loop {
            let result =
                call_iterator_method_async(caller, next, iterator, value::encode_undefined()).await;
            if value::is_exception(result) {
                return Some(vec![result]);
            }
            let Some(result_ptr) = resolve_handle(caller, result) else {
                set_runtime_error(
                    caller.data(),
                    "TypeError: iterator next must return an object".to_string(),
                );
                return None;
            };
            let done = read_object_property_by_name(caller, result_ptr, "done")
                .map(nanbox_to_bool)
                .unwrap_or(false);
            if done {
                break;
            }
            out.push(
                read_object_property_by_name(caller, result_ptr, "value")
                    .unwrap_or_else(value::encode_undefined),
            );
        }
        Some(out)
    }
}

#[cfg(feature = "managed-heap-v2")]
async fn collect_iterator_object_values_v2_async(
    caller: &mut Caller<'_, RuntimeState>,
    iterator: i64,
) -> Option<Vec<i64>> {
    let iterator = if value::is_iterator(iterator) {
        create_raw_iterator_object(caller, iterator)
    } else {
        iterator
    };
    let next = read_host_data_property_v2(caller, iterator, "next")?;
    if !value::is_callable(next) {
        set_runtime_error(
            caller.data(),
            "TypeError: iterator next is not callable".to_string(),
        );
        return None;
    }
    let mut out = Vec::new();
    loop {
        let result =
            call_iterator_method_async(caller, next, iterator, value::encode_undefined()).await;
        if value::is_exception(result) {
            return Some(vec![result]);
        }
        if !value::is_object(result) {
            set_runtime_error(
                caller.data(),
                "TypeError: iterator next must return an object".to_string(),
            );
            return None;
        }
        let done = read_host_data_property_v2(caller, result, "done")
            .map(nanbox_to_bool)
            .unwrap_or(false);
        if done {
            break;
        }
        out.push(
            read_host_data_property_v2(caller, result, "value")
                .unwrap_or_else(value::encode_undefined),
        );
    }
    Some(out)
}

fn map_entry_pair_from_value(
    caller: &mut Caller<'_, RuntimeState>,
    entry_val: i64,
) -> Option<(i64, i64)> {
    if !value::is_js_object(entry_val) && !value::is_array(entry_val) {
        set_runtime_error(
            caller.data(),
            "TypeError: Iterator value is not an entry object".to_string(),
        );
        return None;
    }
    #[cfg(feature = "managed-heap-v2")]
    {
        if value::is_array(entry_val) {
            let handle = value::decode_handle(entry_val);
            let access = caller.data().heap_access_v2();
            let key = access
                .get_element(handle, 0)
                .ok()
                .flatten()
                .map(|entry| entry as i64)
                .unwrap_or_else(value::encode_undefined);
            let val = access
                .get_element(handle, 1)
                .ok()
                .flatten()
                .map(|entry| entry as i64)
                .unwrap_or_else(value::encode_undefined);
            return Some((key, val));
        }
        let key = read_host_data_property_v2(caller, entry_val, "0")
            .unwrap_or_else(value::encode_undefined);
        let val = read_host_data_property_v2(caller, entry_val, "1")
            .unwrap_or_else(value::encode_undefined);
        Some((key, val))
    }
    #[cfg(not(feature = "managed-heap-v2"))]
    {
        if value::is_array(entry_val) {
            let entry_ptr = resolve_handle(caller, entry_val)?;
            let key = read_array_elem(caller, entry_ptr, 0).unwrap_or_else(value::encode_undefined);
            let val = read_array_elem(caller, entry_ptr, 1).unwrap_or_else(value::encode_undefined);
            return Some((key, val));
        }
        let entry_ptr = resolve_handle(caller, entry_val)?;
        let key = read_object_property_by_name(caller, entry_ptr, "0")
            .unwrap_or_else(value::encode_undefined);
        let val = read_object_property_by_name(caller, entry_ptr, "1")
            .unwrap_or_else(value::encode_undefined);
        Some((key, val))
    }
}

/// 为 Map/Set 创建 keys / values / entries 迭代器（与 NativeCallable 路径共用）。
pub(crate) fn map_set_create_iterator(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: MapSetMethodKind,
) -> i64 {
    if !value::is_object(this_val) {
        set_runtime_error(
            caller.data(),
            "TypeError: Method Map/Set.prototype method called on incompatible receiver"
                .to_string(),
        );
        return value::encode_undefined();
    }
    let (map_handle, set_handle) = collection_handles(caller, this_val);
    if map_handle.is_none() && set_handle.is_none() {
        set_runtime_error(
            caller.data(),
            "TypeError: Method Map/Set.prototype method called on incompatible receiver"
                .to_string(),
        );
        return value::encode_undefined();
    }
    match kind {
        MapSetMethodKind::Keys => {
            if let Some(mh) = map_handle {
                let map_handle_u32 = value::decode_f64(mh) as u32;
                let table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if (map_handle_u32 as usize) < table.len() {
                    drop(table);
                    let mut iters = caller
                        .data()
                        .iterators
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::MapKeyIter {
                        map_handle: map_handle_u32,
                        owner: this_val,
                        index: 0,
                    });
                    let iterator = value::encode_handle(value::TAG_ITERATOR, iter_handle);
                    drop(iters);
                    return create_raw_iterator_object(caller, iterator);
                }
            }
            if let Some(sh) = set_handle {
                let set_handle_u32 = value::decode_f64(sh) as u32;
                let table = caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if (set_handle_u32 as usize) < table.len() {
                    drop(table);
                    let mut iters = caller
                        .data()
                        .iterators
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::SetValueIter {
                        set_handle: set_handle_u32,
                        owner: this_val,
                        index: 0,
                    });
                    let iterator = value::encode_handle(value::TAG_ITERATOR, iter_handle);
                    drop(iters);
                    return create_raw_iterator_object(caller, iterator);
                }
            }
            value::encode_undefined()
        }
        MapSetMethodKind::Values => {
            if let Some(mh) = map_handle {
                let map_handle_u32 = value::decode_f64(mh) as u32;
                let table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if (map_handle_u32 as usize) < table.len() {
                    drop(table);
                    let mut iters = caller
                        .data()
                        .iterators
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::MapValueIter {
                        map_handle: map_handle_u32,
                        owner: this_val,
                        index: 0,
                    });
                    let iterator = value::encode_handle(value::TAG_ITERATOR, iter_handle);
                    drop(iters);
                    return create_raw_iterator_object(caller, iterator);
                }
            }
            if let Some(sh) = set_handle {
                let set_handle_u32 = value::decode_f64(sh) as u32;
                let table = caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if (set_handle_u32 as usize) < table.len() {
                    drop(table);
                    let mut iters = caller
                        .data()
                        .iterators
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::SetValueIter {
                        set_handle: set_handle_u32,
                        owner: this_val,
                        index: 0,
                    });
                    let iterator = value::encode_handle(value::TAG_ITERATOR, iter_handle);
                    drop(iters);
                    return create_raw_iterator_object(caller, iterator);
                }
            }
            value::encode_undefined()
        }
        MapSetMethodKind::Entries => {
            if let Some(mh) = map_handle {
                let map_handle_u32 = value::decode_f64(mh) as u32;
                let table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if (map_handle_u32 as usize) < table.len() {
                    drop(table);
                    let mut iters = caller
                        .data()
                        .iterators
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::MapEntryIter {
                        map_handle: map_handle_u32,
                        owner: this_val,
                        index: 0,
                    });
                    let iterator = value::encode_handle(value::TAG_ITERATOR, iter_handle);
                    drop(iters);
                    return create_raw_iterator_object(caller, iterator);
                }
            }
            if let Some(sh) = set_handle {
                let set_handle_u32 = value::decode_f64(sh) as u32;
                let table = caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if (set_handle_u32 as usize) < table.len() {
                    drop(table);
                    let mut iters = caller
                        .data()
                        .iterators
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::SetEntryIter {
                        set_handle: set_handle_u32,
                        owner: this_val,
                        index: 0,
                    });
                    let iterator = value::encode_handle(value::TAG_ITERATOR, iter_handle);
                    drop(iters);
                    return create_raw_iterator_object(caller, iterator);
                }
            }
            value::encode_undefined()
        }
        _ => value::encode_undefined(),
    }
}

async fn invoke_collection_callback_async(
    caller: &mut Caller<'_, RuntimeState>,
    callback: i64,
    this_arg: i64,
    args: &[i64],
) -> Option<i64> {
    #[cfg(feature = "managed-heap-v2")]
    {
        return match crate::runtime_host_helpers::call_wasm_callback_async(
            caller, callback, this_arg, args,
        )
        .await
        {
            Ok(result) => Some(result),
            Err(error) => {
                set_runtime_error(
                    caller.data(),
                    format!("host function callback error: {error:#}"),
                );
                None
            }
        };
    }
    #[cfg(not(feature = "managed-heap-v2"))]
    {
        let env = WasmEnv::from_caller(caller)?;
        invoke_resolved_callback_async_option(caller, &env, callback, this_arg, args).await
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
        set_runtime_error(
            caller.data(),
            "TypeError: Method Map/Set.prototype.forEach called on incompatible receiver"
                .to_string(),
        );
        return value::encode_undefined();
    }
    let (map_handle, set_handle) = collection_handles(caller, this_val);
    if map_handle.is_none() && set_handle.is_none() {
        set_runtime_error(
            caller.data(),
            "TypeError: Method Map/Set.prototype.forEach called on incompatible receiver"
                .to_string(),
        );
        return value::encode_undefined();
    }
    let rt = tokio::runtime::Handle::current();
    if let Some(mh) = map_handle {
        let handle = value::decode_f64(mh) as usize;
        let pairs: Vec<(i64, i64)> = {
            let table = caller
                .data()
                .map_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
                .block_on(invoke_collection_callback_async(
                    caller,
                    cb,
                    this_arg,
                    &[val, key, this_val],
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
            let table = caller
                .data()
                .set_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if handle >= table.len() {
                return value::encode_undefined();
            }
            table[handle].values.clone()
        };
        for val in values {
            if rt
                .block_on(invoke_collection_callback_async(
                    caller,
                    cb,
                    this_arg,
                    &[val, val, this_val],
                ))
                .is_none()
            {
                return value::encode_undefined();
            }
        }
        return value::encode_undefined();
    }
    set_runtime_error(
        caller.data(),
        "TypeError: Method Map/Set.prototype.forEach called on incompatible receiver".to_string(),
    );
    value::encode_undefined()
}

/// Map/Set.prototype.forEach：异步宿主调用路径，避免在运行时内嵌套 block_on。
pub(crate) async fn map_set_for_each_impl_async(
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
        set_runtime_error(
            caller.data(),
            "TypeError: Method Map/Set.prototype.forEach called on incompatible receiver"
                .to_string(),
        );
        return value::encode_undefined();
    }
    let (map_handle, set_handle) = collection_handles(caller, this_val);
    if map_handle.is_none() && set_handle.is_none() {
        set_runtime_error(
            caller.data(),
            "TypeError: Method Map/Set.prototype.forEach called on incompatible receiver"
                .to_string(),
        );
        return value::encode_undefined();
    }
    if let Some(mh) = map_handle {
        let handle = value::decode_f64(mh) as usize;
        let pairs: Vec<(i64, i64)> = {
            let table = caller
                .data()
                .map_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
            if invoke_collection_callback_async(caller, cb, this_arg, &[val, key, this_val])
                .await
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
            let table = caller
                .data()
                .set_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if handle >= table.len() {
                return value::encode_undefined();
            }
            table[handle].values.clone()
        };
        for val in values {
            if invoke_collection_callback_async(caller, cb, this_arg, &[val, val, this_val])
                .await
                .is_none()
            {
                return value::encode_undefined();
            }
        }
        return value::encode_undefined();
    }
    set_runtime_error(
        caller.data(),
        "TypeError: Method Map/Set.prototype.forEach called on incompatible receiver".to_string(),
    );
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
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) =
                    Some("TypeError: Invalid value used as weak map key".to_string());
                return this_val;
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller
                    .data()
                    .weakmap_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
                .weakmap_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
                .weakmap_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
                .weakmap_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) =
                    Some("TypeError: Invalid value used in weak set".to_string());
                return this_val;
            }
            let handle = read_weakset_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller
                    .data()
                    .weakset_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
                .weakset_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
                .weakset_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
    let (map_handle, set_handle) = collection_handles(caller, this_val);

    match kind {
        MapSetMethodKind::MapSet => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let val = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let mut table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    let entry = &mut table[handle];
                    for i in 0..entry.keys.len() {
                        if same_value_zero(caller, entry.keys[i], key) {
                            entry.values[i] = val;
                            return this_val;
                        }
                    }
                    entry.keys.push(key);
                    entry.values.push(val);
                }
                return this_val;
            }
            set_runtime_error(
                caller.data(),
                "TypeError: Method Map.prototype.set called on incompatible receiver".to_string(),
            );
            this_val
        }
        MapSetMethodKind::MapGet => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    let entry = &table[handle];
                    for i in 0..entry.keys.len() {
                        if same_value_zero(caller, entry.keys[i], key) {
                            return entry.values[i];
                        }
                    }
                }
                return value::encode_undefined();
            }
            set_runtime_error(
                caller.data(),
                "TypeError: Method Map.prototype.get called on incompatible receiver".to_string(),
            );
            value::encode_undefined()
        }
        MapSetMethodKind::SetAdd => {
            let val = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let mut table = caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    let entry = &mut table[handle];
                    for i in 0..entry.values.len() {
                        if same_value_zero(caller, entry.values[i], val) {
                            return this_val;
                        }
                    }
                    entry.values.push(val);
                }
                return this_val;
            }
            set_runtime_error(
                caller.data(),
                "TypeError: Method Set.prototype.add called on incompatible receiver".to_string(),
            );
            this_val
        }
        MapSetMethodKind::Has => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    let entry = &table[handle];
                    for i in 0..entry.keys.len() {
                        if same_value_zero(caller, entry.keys[i], key) {
                            return value::encode_bool(true);
                        }
                    }
                }
                return value::encode_bool(false);
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let table = caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    let entry = &table[handle];
                    for i in 0..entry.values.len() {
                        if same_value_zero(caller, entry.values[i], key) {
                            return value::encode_bool(true);
                        }
                    }
                }
                return value::encode_bool(false);
            }
            set_runtime_error(
                caller.data(),
                "TypeError: Method Map/Set.prototype.has called on incompatible receiver"
                    .to_string(),
            );
            value::encode_bool(false)
        }
        MapSetMethodKind::Delete => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let mut table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    let entry = &mut table[handle];
                    for i in 0..entry.keys.len() {
                        if same_value_zero(caller, entry.keys[i], key) {
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
                let mut table = caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    let entry = &mut table[handle];
                    for i in 0..entry.values.len() {
                        if same_value_zero(caller, entry.values[i], key) {
                            entry.values.remove(i);
                            return value::encode_bool(true);
                        }
                    }
                }
                return value::encode_bool(false);
            }
            set_runtime_error(
                caller.data(),
                "TypeError: Method Map/Set.prototype.delete called on incompatible receiver"
                    .to_string(),
            );
            value::encode_bool(false)
        }
        MapSetMethodKind::Clear => {
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let mut table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    table[handle].keys.clear();
                    table[handle].values.clear();
                }
                return value::encode_undefined();
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let mut table = caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    table[handle].values.clear();
                }
                return value::encode_undefined();
            }
            set_runtime_error(
                caller.data(),
                "TypeError: Method Map/Set.prototype.clear called on incompatible receiver"
                    .to_string(),
            );
            value::encode_undefined()
        }
        MapSetMethodKind::Size => {
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    return value::encode_f64(table[handle].keys.len() as f64);
                }
                return value::encode_f64(0.0);
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let table = caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if handle < table.len() {
                    return value::encode_f64(table[handle].values.len() as f64);
                }
                return value::encode_f64(0.0);
            }
            set_runtime_error(
                caller.data(),
                "TypeError: Method Map/Set.prototype.size called on incompatible receiver"
                    .to_string(),
            );
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
