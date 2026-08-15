use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{
    fail_dispatch, get_property, iterator_close, iterator_done, iterator_from, iterator_value,
    strict_equal, type_error,
};
use crate::{BUILTIN_PROTOTYPE_PROPERTY_FLAGS, NativeAgentState, NativeCallableKind};

#[derive(Clone, Copy)]
pub(super) enum CollectionIteratorKind {
    Entries,
    Keys,
    Values,
}

#[derive(Clone, Copy)]
pub(super) enum CollectionIteratorSource {
    Map(u32),
    Set(u32),
    TypedArray(u32),
}

#[derive(Clone, Copy)]
pub(crate) struct CollectionIterator {
    pub(super) source: CollectionIteratorSource,
    pub(super) kind: CollectionIteratorKind,
    pub(super) index: usize,
}

pub(super) fn dispatch_collection(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::MapConstructor => construct_map(ctx, state, args),
        Builtin::MapGroupBy => map_group_by(ctx, state, args),
        Builtin::MapProtoSet => map_set(ctx, state, args),
        Builtin::MapProtoGet => map_get(ctx, state, args),
        Builtin::SetConstructor => construct_set(ctx, state, args),
        Builtin::SetProtoAdd => set_add(ctx, state, args),
        Builtin::SetProtoHas => set_has(ctx, state, args),
        Builtin::SetProtoDelete => set_delete(ctx, state, args),
        Builtin::MapSetHas => map_set_has(ctx, state, args),
        Builtin::MapSetDelete => map_set_delete(ctx, state, args),
        Builtin::MapSetClear => map_set_clear(ctx, state, args),
        Builtin::MapSetGetSize => map_set_size(ctx, state, args),
        Builtin::MapSetForEach => map_set_for_each(ctx, state, args),
        Builtin::MapSetKeys => iterator(ctx, state, args, CollectionIteratorKind::Keys),
        Builtin::MapSetValues => iterator(ctx, state, args, CollectionIteratorKind::Values),
        Builtin::MapSetEntries => iterator(ctx, state, args, CollectionIteratorKind::Entries),
        Builtin::MapSetFirstKey => first_key(ctx, state, args),
        _ => return None,
    })
}

pub(crate) enum CollectionProperty {
    Method(Builtin),
    Value(i64),
}

pub(crate) fn property(
    state: &NativeAgentState,
    receiver: i64,
    key: &str,
) -> Option<CollectionProperty> {
    let handle = value::decode_handle(receiver);
    if let Some(entries) = state.maps.get(&handle) {
        return Some(match key {
            "clear" => CollectionProperty::Method(Builtin::MapSetClear),
            "delete" => CollectionProperty::Method(Builtin::MapSetDelete),
            "entries" => CollectionProperty::Method(Builtin::MapSetEntries),
            "forEach" => CollectionProperty::Method(Builtin::MapSetForEach),
            "get" => CollectionProperty::Method(Builtin::MapProtoGet),
            "has" => CollectionProperty::Method(Builtin::MapSetHas),
            "keys" => CollectionProperty::Method(Builtin::MapSetKeys),
            "set" => CollectionProperty::Method(Builtin::MapProtoSet),
            "size" => CollectionProperty::Value(value::encode_f64(entries.len() as f64)),
            "values" => CollectionProperty::Method(Builtin::MapSetValues),
            _ => return None,
        });
    }
    if let Some(values) = state.sets.get(&handle) {
        return Some(match key {
            "add" => CollectionProperty::Method(Builtin::SetProtoAdd),
            "clear" => CollectionProperty::Method(Builtin::MapSetClear),
            "delete" => CollectionProperty::Method(Builtin::SetProtoDelete),
            "entries" => CollectionProperty::Method(Builtin::MapSetEntries),
            "forEach" => CollectionProperty::Method(Builtin::MapSetForEach),
            "has" => CollectionProperty::Method(Builtin::SetProtoHas),
            "keys" | "values" => CollectionProperty::Method(Builtin::MapSetValues),
            "size" => CollectionProperty::Value(value::encode_f64(values.len() as f64)),
            _ => return None,
        });
    }
    None
}

pub(crate) fn install_prototype_methods(
    state: &mut NativeAgentState,
    prototype: i64,
    is_set: bool,
) -> Result<(), ()> {
    let methods: &[(&str, Builtin)] = if is_set {
        &[
            ("add", Builtin::SetProtoAdd),
            ("clear", Builtin::MapSetClear),
            ("delete", Builtin::SetProtoDelete),
            ("entries", Builtin::MapSetEntries),
            ("forEach", Builtin::MapSetForEach),
            ("has", Builtin::SetProtoHas),
            ("keys", Builtin::MapSetValues),
            ("values", Builtin::MapSetValues),
        ]
    } else {
        &[
            ("clear", Builtin::MapSetClear),
            ("delete", Builtin::MapSetDelete),
            ("entries", Builtin::MapSetEntries),
            ("forEach", Builtin::MapSetForEach),
            ("get", Builtin::MapProtoGet),
            ("has", Builtin::MapSetHas),
            ("keys", Builtin::MapSetKeys),
            ("set", Builtin::MapProtoSet),
            ("values", Builtin::MapSetValues),
        ]
    };
    let prototype = value::decode_handle(prototype);
    for &(name, builtin) in methods {
        let key = state
            .intern_text(name.into(), value::TAG_STRING)
            .map(value::decode_handle)
            .ok_or(())?;
        let callable = state
            .native_callable(NativeCallableKind::Builtin(builtin, true))
            .ok_or(())?;
        state
            .heap
            .set_property(prototype, key, callable as u64)
            .map_err(|_| ())?;
        state
            .heap
            .update_property_flags(prototype, key, BUILTIN_PROTOTYPE_PROPERTY_FLAGS)
            .map_err(|_| ())?;
    }
    Ok(())
}

pub(crate) fn iterator_property(state: &NativeAgentState, receiver: i64, key: &str) -> Option<i64> {
    if !value::is_js_object(receiver) || key != "next" {
        return None;
    }
    let handle = value::decode_handle(receiver);
    state
        .iterator_next
        .get(&handle)
        .copied()
        .map(value::encode_native_callable_idx)
}

pub(crate) fn next(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator_id: u32,
) -> i64 {
    let Some(iterator) = state
        .collection_iterators
        .get(usize::try_from(iterator_id).unwrap_or(usize::MAX))
        .copied()
    else {
        return fail_dispatch(ctx);
    };
    let (next_value, done) = match iterator.source {
        CollectionIteratorSource::Map(handle) => {
            let Some(entries) = state.maps.get(&handle) else {
                return fail_dispatch(ctx);
            };
            match entries.get(iterator.index).copied() {
                None => (value::encode_undefined(), true),
                Some((key, stored)) => match iterator.kind {
                    CollectionIteratorKind::Keys => (key, false),
                    CollectionIteratorKind::Values => (stored, false),
                    CollectionIteratorKind::Entries => {
                        let Ok(entry) = state.allocate_array_values(&[key, stored]) else {
                            return fail_dispatch(ctx);
                        };
                        (entry, false)
                    }
                },
            }
        }
        CollectionIteratorSource::Set(handle) => {
            let Some(values) = state.sets.get(&handle) else {
                return fail_dispatch(ctx);
            };
            match values.get(iterator.index).copied() {
                None => (value::encode_undefined(), true),
                Some(stored) => match iterator.kind {
                    CollectionIteratorKind::Keys | CollectionIteratorKind::Values => {
                        (stored, false)
                    }
                    CollectionIteratorKind::Entries => {
                        let Ok(entry) = state.allocate_array_values(&[stored, stored]) else {
                            return fail_dispatch(ctx);
                        };
                        (entry, false)
                    }
                },
            }
        }
        CollectionIteratorSource::TypedArray(handle) => {
            let Some(array) = state.typed_arrays.get(&handle) else {
                return fail_dispatch(ctx);
            };
            if iterator.index >= array.length {
                (value::encode_undefined(), true)
            } else {
                let stored = super::typedarray::get_element(
                    state,
                    value::encode_object_handle(handle),
                    iterator.index,
                )
                .unwrap_or_else(value::encode_undefined);
                match iterator.kind {
                    CollectionIteratorKind::Keys => {
                        (value::encode_f64(iterator.index as f64), false)
                    }
                    CollectionIteratorKind::Values => (stored, false),
                    CollectionIteratorKind::Entries => {
                        let Ok(entry) = state.allocate_array_values(&[
                            value::encode_f64(iterator.index as f64),
                            stored,
                        ]) else {
                            return fail_dispatch(ctx);
                        };
                        (entry, false)
                    }
                }
            }
        }
    };
    if !done {
        if let Some(iterator) = state
            .collection_iterators
            .get_mut(usize::try_from(iterator_id).unwrap_or(usize::MAX))
        {
            iterator.index = iterator.index.saturating_add(1);
        }
    }
    let Ok(result) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(result);
    for (name, stored) in [("value", next_value), ("done", value::encode_bool(done))] {
        let Some(key) = state.intern_text(name.into(), value::TAG_STRING) else {
            return fail_dispatch(ctx);
        };
        if state
            .heap
            .set_property(handle, value::decode_handle(key), stored as u64)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    result
}

fn collection_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    constructor: Builtin,
) -> Option<i64> {
    let object = state
        .allocate_object(4, false)
        .map_err(|_| fail_dispatch(ctx))
        .ok()?;
    state
        .set_collection_prototype(object, constructor)
        .map_err(|()| fail_dispatch(ctx))
        .ok()?;
    Some(object)
}

fn construct_map(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(object) = collection_object(ctx, state, Builtin::MapConstructor) else {
        return fail_dispatch(ctx);
    };
    state.maps.insert(value::decode_handle(object), Vec::new());
    let iterable = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_null(iterable) || value::is_undefined(iterable) {
        return object;
    }
    let iterator = iterator_from(ctx, state, &[iterable]);
    if value::is_exception(iterator) {
        return iterator;
    }
    loop {
        let done = iterator_done(ctx, state, &[iterator]);
        if value::is_exception(done) {
            return done;
        }
        if super::runtime::is_truthy(state, done) {
            return object;
        }
        let entry = iterator_value(ctx, state, &[iterator], true);
        if value::is_exception(entry) {
            return entry;
        }
        if !(value::is_object(entry) || value::is_array(entry)) {
            return type_error(ctx, state, "Map iterator value is not an entry object");
        }
        let key = match get_property(ctx, state, entry, value::encode_f64(0.0)) {
            Ok(key) => key,
            Err(()) => return fail_dispatch(ctx),
        };
        let stored = match get_property(ctx, state, entry, value::encode_f64(1.0)) {
            Ok(stored) => stored,
            Err(()) => return fail_dispatch(ctx),
        };
        map_insert(state, object, key, stored);
    }
}

fn map_group_by(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [source, callback] = args else {
        return fail_dispatch(ctx);
    };
    if value::is_null(*source) || value::is_undefined(*source) {
        return type_error(ctx, state, "Cannot group null or undefined");
    }
    if !state.is_callable_value(*callback) {
        return type_error(ctx, state, "callbackfn is not callable");
    }
    let iterator = iterator_from(ctx, state, &[*source]);
    if value::is_exception(iterator) {
        return iterator;
    }
    let Some(result) = collection_object(ctx, state, Builtin::MapConstructor) else {
        let exception = fail_dispatch(ctx);
        return iterator_close(ctx, state, &[iterator, exception]);
    };
    state.maps.insert(value::decode_handle(result), Vec::new());
    let mut index = 0_u64;
    loop {
        let done = iterator_done(ctx, state, &[iterator]);
        if value::is_exception(done) {
            return iterator_close(ctx, state, &[iterator, done]);
        }
        if super::runtime::is_truthy(state, done) {
            return result;
        }
        let stored = iterator_value(ctx, state, &[iterator], true);
        if value::is_exception(stored) {
            return iterator_close(ctx, state, &[iterator, stored]);
        }
        if index >= 9_007_199_254_740_991 {
            let exception = type_error(
                ctx,
                state,
                "Map.groupBy index exceeds Number.MAX_SAFE_INTEGER",
            );
            return iterator_close(ctx, state, &[iterator, exception]);
        }
        let key = state
            .invoke_callable(
                ctx,
                *callback,
                value::encode_undefined(),
                &[stored, value::encode_f64(index as f64)],
            )
            .unwrap_or_else(|| fail_dispatch(ctx));
        if value::is_exception(key) {
            return iterator_close(ctx, state, &[iterator, key]);
        }
        let key = canonicalize_keyed_collection_key(key);
        let group = map_get(ctx, state, &[result, key]);
        let group = if value::is_undefined(group) {
            let Ok(group) = state.allocate_array_values(&[]) else {
                let exception = fail_dispatch(ctx);
                return iterator_close(ctx, state, &[iterator, exception]);
            };
            map_insert(state, result, key, group);
            group
        } else {
            group
        };
        if state
            .heap
            .push_element(value::decode_handle(group), stored as u64)
            .is_err()
        {
            let exception = fail_dispatch(ctx);
            return iterator_close(ctx, state, &[iterator, exception]);
        }
        index += 1;
    }
}

fn construct_set(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(object) = collection_object(ctx, state, Builtin::SetConstructor) else {
        return fail_dispatch(ctx);
    };
    state.sets.insert(value::decode_handle(object), Vec::new());
    let iterable = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_null(iterable) || value::is_undefined(iterable) {
        return object;
    }
    let iterator = iterator_from(ctx, state, &[iterable]);
    if value::is_exception(iterator) {
        return iterator;
    }
    loop {
        let done = iterator_done(ctx, state, &[iterator]);
        if value::is_exception(done) {
            return done;
        }
        if super::runtime::is_truthy(state, done) {
            return object;
        }
        let stored = iterator_value(ctx, state, &[iterator], true);
        if value::is_exception(stored) {
            return stored;
        }
        set_insert(state, object, stored);
    }
}

fn map_set(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, key, stored] = args else {
        return fail_dispatch(ctx);
    };
    if !state.maps.contains_key(&value::decode_handle(*receiver)) {
        return fail_dispatch(ctx);
    }
    map_insert(state, *receiver, *key, *stored);
    *receiver
}

fn map_get(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, key] = args else {
        return fail_dispatch(ctx);
    };
    state
        .maps
        .get(&value::decode_handle(*receiver))
        .and_then(|entries| {
            entries
                .iter()
                .find(|(candidate, _)| same_value_zero(state, *candidate, *key))
        })
        .map_or_else(|| value::encode_undefined(), |(_, stored)| *stored)
}

fn set_add(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, stored] = args else {
        return fail_dispatch(ctx);
    };
    if !state.sets.contains_key(&value::decode_handle(*receiver)) {
        return fail_dispatch(ctx);
    }
    set_insert(state, *receiver, *stored);
    *receiver
}

fn set_has(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, stored] = args else {
        return fail_dispatch(ctx);
    };
    let found = state
        .sets
        .get(&value::decode_handle(*receiver))
        .is_some_and(|values| {
            values
                .iter()
                .any(|candidate| same_value_zero(state, *candidate, *stored))
        });
    value::encode_bool(found)
}

fn set_delete(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, stored] = args else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(*receiver);
    let Some(index) = state.sets.get(&handle).and_then(|values| {
        values
            .iter()
            .position(|candidate| same_value_zero(state, *candidate, *stored))
    }) else {
        return value::encode_bool(false);
    };
    let Some(values) = state.sets.get_mut(&handle) else {
        return fail_dispatch(ctx);
    };
    values.remove(index);
    value::encode_bool(true)
}

fn map_set_has(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, key] = args else {
        return fail_dispatch(ctx);
    };
    let found = state
        .maps
        .get(&value::decode_handle(*receiver))
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|(candidate, _)| same_value_zero(state, *candidate, *key))
        })
        || state
            .sets
            .get(&value::decode_handle(*receiver))
            .is_some_and(|values| {
                values
                    .iter()
                    .any(|candidate| same_value_zero(state, *candidate, *key))
            });
    value::encode_bool(found)
}

fn map_set_delete(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, key] = args else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(*receiver);
    if let Some(index) = state.maps.get(&handle).and_then(|entries| {
        entries
            .iter()
            .position(|(candidate, _)| same_value_zero(state, *candidate, *key))
    }) {
        let Some(entries) = state.maps.get_mut(&handle) else {
            return fail_dispatch(ctx);
        };
        entries.remove(index);
        return value::encode_bool(true);
    }
    if state.maps.contains_key(&handle) {
        return value::encode_bool(false);
    }
    set_delete(ctx, state, args)
}

fn map_set_clear(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    if let Some(entries) = state.maps.get_mut(&value::decode_handle(receiver)) {
        entries.clear();
    } else if let Some(values) = state.sets.get_mut(&value::decode_handle(receiver)) {
        values.clear();
    } else {
        return fail_dispatch(ctx);
    }
    value::encode_undefined()
}

fn map_set_size(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    state.maps.get(&value::decode_handle(receiver)).map_or_else(
        || {
            state.sets.get(&value::decode_handle(receiver)).map_or_else(
                || fail_dispatch(ctx),
                |values| value::encode_f64(values.len() as f64),
            )
        },
        |entries| value::encode_f64(entries.len() as f64),
    )
}

fn map_set_for_each(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, callback, this_value] = [
        args.first()
            .copied()
            .unwrap_or_else(value::encode_undefined),
        args.get(1).copied().unwrap_or_else(value::encode_undefined),
        args.get(2).copied().unwrap_or_else(value::encode_undefined),
    ];
    if !value::is_callable(callback) {
        return fail_dispatch(ctx);
    }
    if let Some(entries) = state.maps.get(&value::decode_handle(receiver)).cloned() {
        for (key, stored) in entries {
            let result = state
                .invoke_callable(ctx, callback, this_value, &[stored, key, receiver])
                .unwrap_or_else(|| fail_dispatch(ctx));
            if value::is_exception(result) {
                return result;
            }
        }
    } else if let Some(values) = state.sets.get(&value::decode_handle(receiver)).cloned() {
        for stored in values {
            let result = state
                .invoke_callable(ctx, callback, this_value, &[stored, stored, receiver])
                .unwrap_or_else(|| fail_dispatch(ctx));
            if value::is_exception(result) {
                return result;
            }
        }
    } else {
        return fail_dispatch(ctx);
    }
    value::encode_undefined()
}

fn iterator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: CollectionIteratorKind,
) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let source = if state.maps.contains_key(&value::decode_handle(receiver)) {
        CollectionIteratorSource::Map(value::decode_handle(receiver))
    } else if state.sets.contains_key(&value::decode_handle(receiver)) {
        CollectionIteratorSource::Set(value::decode_handle(receiver))
    } else {
        return fail_dispatch(ctx);
    };
    let Ok(iterator_object) = state.allocate_object(1, false) else {
        return fail_dispatch(ctx);
    };
    let Ok(iterator_id) = u32::try_from(state.collection_iterators.len()) else {
        return fail_dispatch(ctx);
    };
    state.collection_iterators.push(CollectionIterator {
        source,
        kind,
        index: 0,
    });
    let Some(next) = state.native_callable(NativeCallableKind::CollectionNext(iterator_id)) else {
        return fail_dispatch(ctx);
    };
    state.iterator_next.insert(
        value::decode_handle(iterator_object),
        value::decode_native_callable_idx(next),
    );
    iterator_object
}

fn first_key(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    state
        .maps
        .get(&value::decode_handle(receiver))
        .and_then(|entries| entries.first().map(|(key, _)| *key))
        .unwrap_or_else(value::encode_undefined)
}

fn canonicalize_keyed_collection_key(key: i64) -> i64 {
    if value::is_f64(key) && value::decode_f64(key) == 0.0 {
        value::encode_f64(0.0)
    } else {
        key
    }
}

fn map_insert(state: &mut NativeAgentState, receiver: i64, key: i64, stored: i64) {
    let key = canonicalize_keyed_collection_key(key);
    let handle = value::decode_handle(receiver);
    let index = state.maps.get(&handle).and_then(|entries| {
        entries
            .iter()
            .position(|(candidate, _)| same_value_zero(state, *candidate, key))
    });
    let Some(entries) = state.maps.get_mut(&handle) else {
        return;
    };
    if let Some(index) = index {
        entries[index].1 = stored;
    } else {
        entries.push((key, stored));
    }
}

fn set_insert(state: &mut NativeAgentState, receiver: i64, stored: i64) {
    let stored = canonicalize_keyed_collection_key(stored);
    let handle = value::decode_handle(receiver);
    let exists = state.sets.get(&handle).is_some_and(|values| {
        values
            .iter()
            .any(|candidate| same_value_zero(state, *candidate, stored))
    });
    let Some(values) = state.sets.get_mut(&handle) else {
        return;
    };
    if !exists {
        values.push(stored);
    }
}

fn same_value_zero(state: &NativeAgentState, left: i64, right: i64) -> bool {
    if value::is_f64(left) && value::is_f64(right) {
        let left = value::decode_f64(left);
        let right = value::decode_f64(right);
        left == right || left.is_nan() && right.is_nan()
    } else {
        strict_equal(state, left, right)
    }
}

pub(crate) fn map_entries(state: &NativeAgentState, encoded: i64) -> Option<Vec<(i64, i64)>> {
    state.maps.get(&value::decode_handle(encoded)).cloned()
}

pub(crate) fn set_values(state: &NativeAgentState, encoded: i64) -> Option<Vec<i64>> {
    state.sets.get(&value::decode_handle(encoded)).cloned()
}

pub(crate) fn create_map(state: &mut NativeAgentState) -> Option<i64> {
    let object = state.allocate_object(4, false).ok()?;
    state
        .set_collection_prototype(object, Builtin::MapConstructor)
        .ok()?;
    state.maps.insert(value::decode_handle(object), Vec::new());
    Some(object)
}

pub(crate) fn create_set(state: &mut NativeAgentState) -> Option<i64> {
    let object = state.allocate_object(4, false).ok()?;
    state
        .set_collection_prototype(object, Builtin::SetConstructor)
        .ok()?;
    state.sets.insert(value::decode_handle(object), Vec::new());
    Some(object)
}

pub(crate) fn insert_map_entries(
    state: &mut NativeAgentState,
    object: i64,
    entries: Vec<(i64, i64)>,
) -> bool {
    if !state.maps.contains_key(&value::decode_handle(object)) {
        return false;
    }
    for (key, value) in entries {
        map_insert(state, object, key, value);
    }
    true
}

pub(crate) fn insert_set_values(
    state: &mut NativeAgentState,
    object: i64,
    values: Vec<i64>,
) -> bool {
    if !state.sets.contains_key(&value::decode_handle(object)) {
        return false;
    }
    for value in values {
        set_insert(state, object, value);
    }
    true
}
