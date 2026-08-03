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
    {
        (
            read_host_data_property_v2(caller, receiver, "__map_handle__"),
            read_host_data_property_v2(caller, receiver, "__set_handle__"),
        )
    }
}

// ── Map/Set 哈希索引核心操作（ExecContext 原语与 NativeCallable 路径共用）──
//
// 存储保持插入顺序的 keys/values 平行 Vec；`index` 为 SameValueZero 稳定哈希 →
// 槽位索引（仅存活键），删除打 tombstone（`deleted` 平行标记）保持顺序，
// tombstone 过半时压缩重建。哈希冲突（不同键同哈希）回退线性扫描保证正确。

/// Map 槽位查找：hash 命中 + SameValueZero 验证；hash 冲突回退存活槽位线性扫描。
fn map_find_slot(
    caller: &Caller<'_, RuntimeState>,
    entry: &crate::MapEntry,
    key: i64,
    hash: u64,
) -> Option<u32> {
    if let Some(&pos) = entry.index.get(&hash) {
        let pos = pos as usize;
        if pos < entry.keys.len()
            && !entry.deleted[pos]
            && same_value_zero(caller, entry.keys[pos], key)
        {
            return Some(pos as u32);
        }
    }
    for (i, &k) in entry.keys.iter().enumerate() {
        if !entry.deleted[i] && same_value_zero(caller, k, key) {
            return Some(i as u32);
        }
    }
    None
}

/// Set 槽位查找（与 Map 平行，仅 values）。
fn set_find_slot(
    caller: &Caller<'_, RuntimeState>,
    entry: &crate::SetEntry,
    value: i64,
    hash: u64,
) -> Option<u32> {
    if let Some(&pos) = entry.index.get(&hash) {
        let pos = pos as usize;
        if pos < entry.values.len()
            && !entry.deleted[pos]
            && same_value_zero(caller, entry.values[pos], value)
        {
            return Some(pos as u32);
        }
    }
    for (i, &v) in entry.values.iter().enumerate() {
        if !entry.deleted[i] && same_value_zero(caller, v, value) {
            return Some(i as u32);
        }
    }
    None
}

/// 压缩：剔除 tombstone 槽位，保持存活元素顺序并重建索引。
fn map_compact(caller: &Caller<'_, RuntimeState>, entry: &mut crate::MapEntry) {
    let live = entry.live_count as usize;
    let mut keys = Vec::with_capacity(live);
    let mut values = Vec::with_capacity(live);
    for i in 0..entry.keys.len() {
        if !entry.deleted[i] {
            keys.push(entry.keys[i]);
            values.push(entry.values[i]);
        }
    }
    entry.keys = keys;
    entry.values = values;
    map_entry_reindex(caller, entry);
}

/// Set 压缩（与 Map 平行）。
fn set_compact(caller: &Caller<'_, RuntimeState>, entry: &mut crate::SetEntry) {
    let live = entry.live_count as usize;
    let mut values = Vec::with_capacity(live);
    for i in 0..entry.values.len() {
        if !entry.deleted[i] {
            values.push(entry.values[i]);
        }
    }
    entry.values = values;
    set_entry_reindex(caller, entry);
}

/// 整体赋值后重建 Map 索引（全部视为存活；worker 反序列化 / 结构化克隆用）。
pub(crate) fn map_entry_reindex<C: AsContext<Data = RuntimeState>>(
    ctx: &C,
    entry: &mut crate::MapEntry,
) {
    entry.index.clear();
    entry.deleted.clear();
    entry.deleted.resize(entry.keys.len(), false);
    entry.live_count = entry.keys.len() as u32;
    entry.deleted_count = 0;
    for (pos, &key) in entry.keys.iter().enumerate() {
        entry
            .index
            .insert(same_value_zero_stable_hash(ctx, key), pos as u32);
    }
}

/// 整体赋值后重建 Set 索引（全部视为存活）。
pub(crate) fn set_entry_reindex<C: AsContext<Data = RuntimeState>>(
    ctx: &C,
    entry: &mut crate::SetEntry,
) {
    entry.index.clear();
    entry.deleted.clear();
    entry.deleted.resize(entry.values.len(), false);
    entry.live_count = entry.values.len() as u32;
    entry.deleted_count = 0;
    for (pos, &value) in entry.values.iter().enumerate() {
        entry
            .index
            .insert(same_value_zero_stable_hash(ctx, value), pos as u32);
    }
}

/// Map.prototype.set 实现。
pub(crate) fn map_set_impl(caller: &mut Caller<'_, RuntimeState>, handle: u32, key: i64, val: i64) {
    let mut table = caller
        .data()
        .map_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(entry) = table.get_mut(handle as usize) else {
        return;
    };
    let hash = same_value_zero_stable_hash(caller, key);
    if let Some(pos) = map_find_slot(caller, entry, key, hash) {
        entry.values[pos as usize] = val;
        return;
    }
    let pos = entry.keys.len() as u32;
    entry.keys.push(key);
    entry.values.push(val);
    entry.deleted.push(false);
    entry.index.insert(hash, pos);
    entry.live_count += 1;
}

/// Map.prototype.get 实现。
pub(crate) fn map_get_impl(
    caller: &Caller<'_, RuntimeState>,
    handle: u32,
    key: i64,
) -> Option<i64> {
    let table = caller
        .data()
        .map_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let entry = table.get(handle as usize)?;
    let hash = same_value_zero_stable_hash(caller, key);
    let pos = map_find_slot(caller, entry, key, hash)?;
    Some(entry.values[pos as usize])
}

/// Set.prototype.add 实现。
pub(crate) fn set_add_impl(caller: &mut Caller<'_, RuntimeState>, handle: u32, value: i64) {
    let mut table = caller
        .data()
        .set_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(entry) = table.get_mut(handle as usize) else {
        return;
    };
    let hash = same_value_zero_stable_hash(caller, value);
    if set_find_slot(caller, entry, value, hash).is_some() {
        return;
    }
    let pos = entry.values.len() as u32;
    entry.values.push(value);
    entry.deleted.push(false);
    entry.index.insert(hash, pos);
    entry.live_count += 1;
}

/// Map/Set.prototype.has 实现。
pub(crate) fn map_set_has_impl(
    caller: &Caller<'_, RuntimeState>,
    handle: u32,
    key: i64,
    is_set: bool,
) -> bool {
    if is_set {
        let table = caller
            .data()
            .set_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get(handle as usize) else {
            return false;
        };
        let hash = same_value_zero_stable_hash(caller, key);
        set_find_slot(caller, entry, key, hash).is_some()
    } else {
        let table = caller
            .data()
            .map_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get(handle as usize) else {
            return false;
        };
        let hash = same_value_zero_stable_hash(caller, key);
        map_find_slot(caller, entry, key, hash).is_some()
    }
}

/// Map/Set.prototype.delete 实现（tombstone + 触发压缩）。
pub(crate) fn map_set_delete_impl(
    caller: &mut Caller<'_, RuntimeState>,
    handle: u32,
    key: i64,
    is_set: bool,
) -> bool {
    if is_set {
        let mut table = caller
            .data()
            .set_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get_mut(handle as usize) else {
            return false;
        };
        let hash = same_value_zero_stable_hash(caller, key);
        let Some(pos) = set_find_slot(caller, entry, key, hash) else {
            return false;
        };
        entry.deleted[pos as usize] = true;
        entry.deleted_count += 1;
        entry.live_count -= 1;
        entry.index.remove(&hash);
        if entry.deleted_count > 64 && entry.deleted_count >= entry.live_count {
            set_compact(caller, entry);
        }
        true
    } else {
        let mut table = caller
            .data()
            .map_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get_mut(handle as usize) else {
            return false;
        };
        let hash = same_value_zero_stable_hash(caller, key);
        let Some(pos) = map_find_slot(caller, entry, key, hash) else {
            return false;
        };
        entry.deleted[pos as usize] = true;
        entry.deleted_count += 1;
        entry.live_count -= 1;
        entry.index.remove(&hash);
        if entry.deleted_count > 64 && entry.deleted_count >= entry.live_count {
            map_compact(caller, entry);
        }
        true
    }
}

/// Map/Set.prototype.clear 实现。
pub(crate) fn map_set_clear_impl(caller: &mut Caller<'_, RuntimeState>, handle: u32, is_set: bool) {
    if is_set {
        let mut table = caller
            .data()
            .set_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(handle as usize) {
            entry.clear_for_reuse();
        }
    } else {
        let mut table = caller
            .data()
            .map_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(handle as usize) {
            entry.clear_for_reuse();
        }
    }
}

/// Map/Set.prototype.size 实现（存活数）。
pub(crate) fn map_set_size_impl(
    caller: &Caller<'_, RuntimeState>,
    handle: u32,
    is_set: bool,
) -> u32 {
    if is_set {
        let table = caller
            .data()
            .set_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle as usize)
            .map(|e| e.live_count)
            .unwrap_or(0)
    } else {
        let table = caller
            .data()
            .map_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle as usize)
            .map(|e| e.live_count)
            .unwrap_or(0)
    }
}

/// Map/Set 存活条目快照（跳过 tombstone）。
pub(crate) fn map_set_entries_snapshot_impl(
    caller: &Caller<'_, RuntimeState>,
    handle: u32,
    is_set: bool,
) -> Vec<(i64, i64)> {
    if is_set {
        let table = caller
            .data()
            .set_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get(handle as usize) else {
            return Vec::new();
        };
        entry
            .values
            .iter()
            .enumerate()
            .filter(|(i, _)| !entry.deleted[*i])
            .map(|(_, &v)| (v, v))
            .collect()
    } else {
        let table = caller
            .data()
            .map_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get(handle as usize) else {
            return Vec::new();
        };
        entry
            .keys
            .iter()
            .zip(entry.values.iter())
            .enumerate()
            .filter(|(i, _)| !entry.deleted[*i])
            .map(|(_, (&k, &v))| (k, v))
            .collect()
    }
}

// fill_map_from_constructor_arg_async / fill_set_from_constructor_arg_async /
// collect_constructor_iterable_values_async / collect_iterator_object_values_async /
// collect_iterator_object_values_v2_async / map_entry_pair_from_value
// 已迁移到 wjsm-builtins::iterable_collect + collections，由 ExecContext 原语调用。

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
                .enumerate()
                .filter(|(i, _)| !entry.deleted[*i])
                .map(|(_, (&k, &v))| (k, v))
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
            let entry = &table[handle];
            entry
                .values
                .iter()
                .enumerate()
                .filter(|(i, _)| !entry.deleted[*i])
                .map(|(_, &v)| v)
                .collect()
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
                .enumerate()
                .filter(|(i, _)| !entry.deleted[*i])
                .map(|(_, (&k, &v))| (k, v))
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
            let entry = &table[handle];
            entry
                .values
                .iter()
                .enumerate()
                .filter(|(i, _)| !entry.deleted[*i])
                .map(|(_, &v)| v)
                .collect()
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
                let handle = value::decode_f64(mh) as u32;
                map_set_impl(caller, handle, key, val);
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
                let handle = value::decode_f64(mh) as u32;
                return map_get_impl(caller, handle, key).unwrap_or_else(value::encode_undefined);
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
                let handle = value::decode_f64(sh) as u32;
                set_add_impl(caller, handle, val);
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
                let handle = value::decode_f64(mh) as u32;
                return value::encode_bool(map_set_has_impl(caller, handle, key, false));
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as u32;
                return value::encode_bool(map_set_has_impl(caller, handle, key, true));
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
                let handle = value::decode_f64(mh) as u32;
                return value::encode_bool(map_set_delete_impl(caller, handle, key, false));
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as u32;
                return value::encode_bool(map_set_delete_impl(caller, handle, key, true));
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
                let handle = value::decode_f64(mh) as u32;
                map_set_clear_impl(caller, handle, false);
                return value::encode_undefined();
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as u32;
                map_set_clear_impl(caller, handle, true);
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
                let handle = value::decode_f64(mh) as u32;
                return value::encode_f64(map_set_size_impl(caller, handle, false) as f64);
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as u32;
                return value::encode_f64(map_set_size_impl(caller, handle, true) as f64);
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
