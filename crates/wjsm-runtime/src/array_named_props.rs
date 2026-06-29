//! 数组实例上的非索引命名属性（含 symbol），与元素存储分离。
//! ECMAScript 允许在数组对象上定义 `@@isConcatSpreadable` 等 own 属性。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::property_key::is_symbol_name_id;
use crate::{RuntimeState, value};

#[derive(Clone, Default)]
pub(crate) struct ArrayNamedPropsStore(Arc<Mutex<HashMap<u32, Vec<(u32, i64)>>>>);

impl ArrayNamedPropsStore {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    fn handle_of(_caller: &Caller<'_, RuntimeState>, boxed: i64) -> Option<u32> {
        if !value::is_array(boxed) {
            return None;
        }
        Some(value::decode_handle(boxed))
    }

    pub(crate) fn get(caller: &Caller<'_, RuntimeState>, boxed: i64, name_id: u32) -> i64 {
        let Some(handle) = Self::handle_of(caller, boxed) else {
            return value::encode_undefined();
        };
        let table = caller
            .data()
            .array_named_props
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(&handle)
            .and_then(|slots| slots.iter().find(|(id, _)| *id == name_id).map(|(_, v)| *v))
            .unwrap_or_else(value::encode_undefined)
    }

    pub(crate) fn set(caller: &mut Caller<'_, RuntimeState>, boxed: i64, name_id: u32, val: i64) {
        let Some(handle) = Self::handle_of(caller, boxed) else {
            return;
        };
        let mut table = caller
            .data()
            .array_named_props
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let slots = table.entry(handle).or_default();
        if let Some(slot) = slots.iter_mut().find(|(id, _)| *id == name_id) {
            slot.1 = val;
        } else {
            slots.push((name_id, val));
        }
    }

    /// GC：收集侧表持有的所有值作为根。
    pub(crate) fn trace_roots(store: &ArrayNamedPropsStore, roots: &mut Vec<i64>) {
        let table = store.0.lock().unwrap_or_else(|e| e.into_inner());
        for slots in table.values() {
            for (_, v) in slots {
                if value::tag_needs_root(*v) {
                    roots.push(*v);
                }
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn drop_handle(store: &ArrayNamedPropsStore, handle: u32) {
        let mut table = store.0.lock().unwrap_or_else(|e| e.into_inner());
        table.remove(&handle);
    }
}

pub(crate) fn define_array_named_props(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
) -> anyhow::Result<()> {
    let get_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, boxed: i64, name_id: i32| -> i64 {
            ArrayNamedPropsStore::get(&caller, boxed, name_id as u32)
        },
    );
    linker.define(&mut *store, "env", "array_named_get", get_fn)?;

    let set_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, boxed: i64, name_id: i32, val: i64| {
            ArrayNamedPropsStore::set(&mut caller, boxed, name_id as u32, val);
        },
    );
    linker.define(&mut *store, "env", "array_named_set", set_fn)?;
    Ok(())
}

/// 供 `IsConcatSpreadable` / `Get` 在数组上读取命名属性（含 symbol）。
pub(crate) fn array_named_get_sync(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    name_id: u32,
) -> i64 {
    if value::is_array(arr) {
        return ArrayNamedPropsStore::get(caller, arr, name_id);
    }
    value::encode_undefined()
}

#[allow(dead_code)]
pub(crate) fn array_named_set_sync(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    name_id: u32,
    val: i64,
) {
    if value::is_array(arr) {
        ArrayNamedPropsStore::set(caller, arr, name_id, val);
    }
}

/// 数组上 symbol 等非索引命名属性的 DefineOwnProperty（数据描述符）。
pub(crate) fn define_data_property_on_array_named(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    name_id: u32,
    val: i64,
) -> Result<bool, String> {
    if !value::is_array(arr) {
        return Err("TypeError: target is not an array".to_string());
    }
    if !is_symbol_name_id(name_id) {
        return Err("TypeError: invalid property key on array".to_string());
    }
    ArrayNamedPropsStore::set(caller, arr, name_id, val);
    Ok(true)
}
