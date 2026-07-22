//! 数组实例上的非索引命名属性（含 symbol），与元素存储分离。
//! ECMAScript 允许在数组对象上定义 `@@isConcatSpreadable` 等 own 属性。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};
use wjsm_ir::constants;

use crate::property_key::{is_symbol_name_id, name_id_to_property_key_value};
use crate::runtime_render::store_runtime_string;
use crate::{RuntimeState, value};

#[derive(Clone, Copy)]
pub(crate) struct ArrayNamedPropSlot {
    pub name_id: u32,
    pub value: i64,
    pub flags: i32,
}

#[derive(Clone, Default)]
pub(crate) struct ArrayNamedPropsStore(Arc<Mutex<HashMap<u32, Vec<ArrayNamedPropSlot>>>>);

fn default_data_property_flags() -> i32 {
    constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE
}

/// V2：属性键统一规范化（memory string → interned runtime string），使编译期
/// name_id 与宿主 intern key 落在同一键空间；symbol / runtime id 原样保留。
fn canonical_name_id(caller: &mut Caller<'_, RuntimeState>, name_id: u32) -> u32 {
    crate::property_key::canonicalize_v2_name_id(caller, name_id).unwrap_or(name_id)
}

/// V1：memory c-string 偏移即规范键（find/alloc 去重），无需转换。

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

    pub(crate) fn get(caller: &mut Caller<'_, RuntimeState>, boxed: i64, name_id: u32) -> i64 {
        let Some(handle) = Self::handle_of(caller, boxed) else {
            return value::encode_undefined();
        };
        let name_id = canonical_name_id(caller, name_id);
        let table = caller
            .data()
            .array_named_props
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(&handle)
            .and_then(|slots| {
                slots
                    .iter()
                    .find(|slot| slot.name_id == name_id)
                    .map(|slot| slot.value)
            })
            .unwrap_or_else(value::encode_undefined)
    }

    pub(crate) fn get_slot(
        caller: &mut Caller<'_, RuntimeState>,
        boxed: i64,
        name_id: u32,
    ) -> Option<ArrayNamedPropSlot> {
        let handle = Self::handle_of(caller, boxed)?;
        let name_id = canonical_name_id(caller, name_id);
        let table = caller
            .data()
            .array_named_props
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(&handle)
            .and_then(|slots| slots.iter().find(|slot| slot.name_id == name_id).copied())
    }

    pub(crate) fn set(caller: &mut Caller<'_, RuntimeState>, boxed: i64, name_id: u32, val: i64) {
        Self::set_with_flags(caller, boxed, name_id, val, default_data_property_flags());
    }

    pub(crate) fn set_with_flags(
        caller: &mut Caller<'_, RuntimeState>,
        boxed: i64,
        name_id: u32,
        val: i64,
        flags: i32,
    ) {
        let Some(handle) = Self::handle_of(caller, boxed) else {
            return;
        };
        let name_id = canonical_name_id(caller, name_id);
        let mut table = caller
            .data()
            .array_named_props
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let slots = table.entry(handle).or_default();
        if let Some(slot) = slots.iter_mut().find(|slot| slot.name_id == name_id) {
            slot.value = val;
            slot.flags = flags;
        } else {
            slots.push(ArrayNamedPropSlot {
                name_id,
                value: val,
                flags,
            });
        }
    }

    /// `delete arr.<named>`：configurable=false → `Some(false)`；删除成功 →
    /// `Some(true)`；属性不存在 → `None`（调用方按规范返回 true）。
    /// V1 编译端 obj_delete 无数组分支，仅 V2 host 路由调用。
    pub(crate) fn remove(
        caller: &mut Caller<'_, RuntimeState>,
        boxed: i64,
        name_id: u32,
    ) -> Option<bool> {
        let handle = Self::handle_of(caller, boxed)?;
        let name_id = canonical_name_id(caller, name_id);
        let mut table = caller
            .data()
            .array_named_props
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let slots = table.get_mut(&handle)?;
        let index = slots.iter().position(|slot| slot.name_id == name_id)?;
        if slots[index].flags & constants::FLAG_CONFIGURABLE == 0 {
            return Some(false);
        }
        slots.remove(index);
        Some(true)
    }

    pub(crate) fn collect_string_name_ids(
        caller: &Caller<'_, RuntimeState>,
        arr: i64,
        enumerable_only: bool,
    ) -> Vec<u32> {
        let Some(handle) = Self::handle_of(caller, arr) else {
            return Vec::new();
        };
        let table = caller
            .data()
            .array_named_props
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(slots) = table.get(&handle) else {
            return Vec::new();
        };
        slots
            .iter()
            .filter(|slot| !is_symbol_name_id(slot.name_id))
            .filter(|slot| !enumerable_only || (slot.flags & constants::FLAG_ENUMERABLE) != 0)
            .map(|slot| slot.name_id)
            .collect()
    }

    /// 收集数组命名 own 属性名（可选仅可枚举；不含 symbol）。
    pub(crate) fn collect_string_property_names(
        caller: &mut Caller<'_, RuntimeState>,
        arr: i64,
        enumerable_only: bool,
    ) -> Vec<String> {
        let Some(handle) = Self::handle_of(caller, arr) else {
            return Vec::new();
        };
        let table = caller
            .data()
            .array_named_props
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(slots) = table.get(&handle) else {
            return Vec::new();
        };
        let name_ids: Vec<u32> = slots
            .iter()
            .filter(|slot| !is_symbol_name_id(slot.name_id))
            .filter(|slot| !enumerable_only || (slot.flags & constants::FLAG_ENUMERABLE) != 0)
            .map(|slot| slot.name_id)
            .collect();
        drop(table);
        let mut names = Vec::new();
        for name_id in name_ids {
            if let Some(name) =
                crate::runtime_host_helpers::name_id_to_runtime_property_string(caller, name_id)
            {
                names.push(name.to_utf8_lossy());
            }
        }
        names
    }

    fn collect_property_key_values_by_handle(
        caller: &mut Caller<'_, RuntimeState>,
        handle: u32,
        symbols_only: bool,
    ) -> Vec<i64> {
        let table = caller
            .data()
            .array_named_props
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(slots) = table.get(&handle) else {
            return Vec::new();
        };
        let (string_name_ids, symbol_name_ids): (Vec<u32>, Vec<u32>) = slots
            .iter()
            .map(|slot| slot.name_id)
            .partition(|name_id| !is_symbol_name_id(*name_id));
        drop(table);

        if symbols_only {
            return symbol_name_ids
                .into_iter()
                .filter_map(name_id_to_property_key_value)
                .collect();
        }

        let mut keys = Vec::with_capacity(string_name_ids.len() + symbol_name_ids.len());
        for name_id in string_name_ids {
            if let Some(name) =
                crate::runtime_host_helpers::name_id_to_runtime_property_string(caller, name_id)
            {
                keys.push(store_runtime_string(caller, name));
            }
        }
        keys.extend(
            symbol_name_ids
                .into_iter()
                .filter_map(name_id_to_property_key_value),
        );
        keys
    }

    pub(crate) fn collect_property_key_values_by_ptr(
        caller: &mut Caller<'_, RuntimeState>,
        arr_ptr: usize,
        symbols_only: bool,
    ) -> Vec<i64> {
        // V2：resolve_array_ptr 返回的 "ptr" 即 V2 handle，直接按 handle 收集。
        if u32::try_from(arr_ptr).is_ok_and(|handle| {
            caller
                .data()
                .heap_access_v2()
                .resolve_handle(handle)
                .is_ok()
        }) {
            return Self::collect_property_key_values_by_handle(
                caller,
                arr_ptr as u32,
                symbols_only,
            );
        }
        let handles: Vec<u32> = caller
            .data()
            .array_named_props
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .copied()
            .collect();
        let Some(env) = crate::wasm_env::WasmEnv::from_caller(caller) else {
            return Vec::new();
        };
        for handle in handles {
            if crate::runtime_values::resolve_handle_idx_with_env(caller, &env, handle as usize)
                == Some(arr_ptr)
            {
                return Self::collect_property_key_values_by_handle(caller, handle, symbols_only);
            }
        }
        Vec::new()
    }

    /// GC：收集侧表持有的所有值作为根。
    pub(crate) fn trace_roots(store: &ArrayNamedPropsStore, roots: &mut Vec<i64>) {
        let table = store.0.lock().unwrap_or_else(|e| e.into_inner());
        for slots in table.values() {
            for slot in slots {
                if value::tag_needs_root(slot.value) {
                    roots.push(slot.value);
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
        |mut caller: Caller<'_, RuntimeState>, boxed: i64, name_id: i32| -> i64 {
            ArrayNamedPropsStore::get(&mut caller, boxed, name_id as u32)
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

/// 数组上非索引命名属性的 DefineOwnProperty（数据描述符）。
pub(crate) fn define_data_property_on_array_named(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    name_id: u32,
    desc: &crate::runtime_host_helpers::PropertyDescriptor,
) -> Result<bool, String> {
    if !value::is_array(arr) {
        return Err("TypeError: target is not an array".to_string());
    }
    let completed = crate::runtime_host_helpers::complete_property_descriptor(desc.clone());
    if crate::runtime_host_helpers::is_accessor_descriptor(&completed) {
        return Err(
            "TypeError: Accessor properties are not supported on array symbol slots".to_string(),
        );
    }
    let val = completed.value.unwrap_or_else(value::encode_undefined);
    let mut flags = 0i32;
    if completed.writable.unwrap_or(false) {
        flags |= constants::FLAG_WRITABLE;
    }
    if completed.enumerable.unwrap_or(false) {
        flags |= constants::FLAG_ENUMERABLE;
    }
    if completed.configurable.unwrap_or(false) {
        flags |= constants::FLAG_CONFIGURABLE;
    }
    ArrayNamedPropsStore::set_with_flags(caller, arr, name_id, val, flags);
    Ok(true)
}
