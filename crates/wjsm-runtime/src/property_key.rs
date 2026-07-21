use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wasmtime::{AsContext, Caller};
use wjsm_ir::{constants, value};

use crate::runtime_string::RuntimeString;
use crate::{RuntimeState, WasmEnv};

/// 运行时属性键表：Vec 保序 + HashMap 做 O(1) intern。
#[derive(Default)]
pub(crate) struct PropertyKeyTable {
    by_index: Vec<RuntimeString>,
    index_of: HashMap<RuntimeString, u32>,
}

impl PropertyKeyTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn index_of(&self, key: &RuntimeString) -> Option<u32> {
        self.index_of.get(key).copied()
    }

    pub(crate) fn push(&mut self, key: RuntimeString) -> u32 {
        if let Some(&idx) = self.index_of.get(&key) {
            return idx;
        }
        let index = self.by_index.len() as u32;
        self.index_of.insert(key.clone(), index);
        self.by_index.push(key);
        index
    }

    pub(crate) fn get(&self, index: u32) -> Option<&RuntimeString> {
        self.by_index.get(index as usize)
    }
}

pub(crate) type SharedPropertyKeyTable = Arc<Mutex<PropertyKeyTable>>;

/// 属性槽 `name_id` 的三种存储来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DecodedNameId {
    MemoryString(u32),
    RuntimeString(u32),
    Symbol(u32),
}

#[inline]
pub(crate) fn encode_string_name_id(string_idx: u32) -> u32 {
    assert!(string_idx <= constants::NAME_ID_INDEX_MASK);
    string_idx
}

#[inline]
pub(crate) fn encode_runtime_string_name_id(index: u32) -> u32 {
    assert!(index <= constants::NAME_ID_INDEX_MASK);
    constants::NAME_ID_RUNTIME_STRING_FLAG | index
}

#[inline]
pub(crate) fn encode_symbol_name_id(symbol_idx: u32) -> u32 {
    assert!(symbol_idx <= constants::NAME_ID_INDEX_MASK);
    constants::NAME_ID_SYMBOL_FLAG | symbol_idx
}

#[inline]
pub(crate) fn is_symbol_name_id(name_id: u32) -> bool {
    matches!(decode_name_id(name_id), DecodedNameId::Symbol(_))
}

#[inline]
pub(crate) fn decode_name_id(name_id: u32) -> DecodedNameId {
    let index = name_id & constants::NAME_ID_INDEX_MASK;
    match name_id & constants::NAME_ID_KIND_MASK {
        constants::NAME_ID_SYMBOL_FLAG => DecodedNameId::Symbol(index),
        constants::NAME_ID_RUNTIME_STRING_FLAG => DecodedNameId::RuntimeString(index),
        _ => DecodedNameId::MemoryString(index),
    }
}

#[inline]
pub(crate) fn name_id_to_property_key_value(name_id: u32) -> Option<i64> {
    match decode_name_id(name_id) {
        DecodedNameId::MemoryString(index) => Some(value::encode_string_ptr(index)),
        DecodedNameId::RuntimeString(index) => Some(value::encode_runtime_string_handle(index)),
        DecodedNameId::Symbol(index) => Some(value::encode_symbol_handle(index)),
    }
}

pub(crate) fn intern_runtime_property_key(state: &RuntimeState, key: RuntimeString) -> u32 {
    let mut keys = state
        .runtime_property_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // 先查 index map（按 UTF-16 内容）；无则 append。
    // RuntimeString 实现 Eq/Hash 走 units。
    if let Some(index) = keys.index_of(&key) {
        return index;
    }
    keys.push(key)
}

pub(crate) fn runtime_property_key_units(
    state: &RuntimeState,
    index: u32,
) -> Option<RuntimeString> {
    let table = state
        .runtime_property_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    table.get(index).cloned()
}

pub(crate) fn name_id_matches_runtime_string<C: AsContext<Data = RuntimeState>>(
    ctx: &C,
    env: &WasmEnv,
    slot_name_id: u32,
    key: &RuntimeString,
) -> bool {
    match decode_name_id(slot_name_id) {
        DecodedNameId::MemoryString(index) => {
            let bytes = crate::runtime_render::read_string_bytes_mem(ctx, &env.memory, index);
            RuntimeString::from_utf8_lossy(&bytes) == *key
        }
        DecodedNameId::RuntimeString(index) => {
            runtime_property_key_units(ctx.as_context().data(), index)
                .is_some_and(|stored| stored == *key)
        }
        DecodedNameId::Symbol(_) => false,
    }
}

#[cfg(feature = "managed-heap-v2")]
pub(crate) fn canonicalize_v2_name_id_with_env<C: AsContext<Data = RuntimeState>>(
    ctx: &C,
    env: &WasmEnv,
    name_id: u32,
) -> Option<u32> {
    match decode_name_id(name_id) {
        DecodedNameId::MemoryString(index) => {
            let bytes = crate::runtime_render::read_string_bytes_mem(ctx, &env.memory, index);
            let key = RuntimeString::from_utf8_lossy(&bytes);
            let index = intern_runtime_property_key(ctx.as_context().data(), key);
            Some(encode_runtime_string_name_id(index))
        }
        DecodedNameId::RuntimeString(_) | DecodedNameId::Symbol(_) => Some(name_id),
    }
}

pub(crate) fn property_key_value_to_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    prop: i64,
    allocate_memory_string: bool,
) -> Option<u32> {
    if let Some(id) = symbol_value_to_name_id(prop) {
        return Some(id);
    }
    if value::is_runtime_string_handle(prop) {
        let key = crate::runtime_values::get_string_value(caller, prop);
        // 优先 intern 到 memory c-string（与编译期 name_id 同形态），
        // 使用 find 缓存避免全堆 memmem；失败再走 runtime property key 表。
        if let Some(key_utf8) = key.to_utf8()
            && !key_utf8.as_bytes().contains(&0)
            && let Some(memory_id) = if allocate_memory_string {
                crate::runtime_host_helpers::find_memory_c_string(caller, &key_utf8)
                    .or_else(|| crate::runtime_host_helpers::alloc_heap_c_string(caller, &key_utf8))
            } else {
                crate::runtime_host_helpers::find_memory_c_string(caller, &key_utf8)
            }
        {
            return Some(encode_string_name_id(memory_id));
        }
        let index = intern_runtime_property_key(caller.data(), key);
        return Some(encode_runtime_string_name_id(index));
    }

    if value::is_string(prop) {
        return Some(encode_string_name_id(value::decode_string_ptr(prop)));
    }
    let prop_name = crate::runtime_render::render_value(caller, prop).ok()?;
    let memory_id = if allocate_memory_string {
        crate::runtime_host_helpers::find_memory_c_string(caller, &prop_name)
            .or_else(|| crate::runtime_host_helpers::alloc_heap_c_string(caller, &prop_name))
    } else {
        crate::runtime_host_helpers::find_memory_c_string(caller, &prop_name)
    }?;
    Some(encode_string_name_id(memory_id))
}

#[inline]
pub(crate) fn symbol_value_to_name_id(symbol_val: i64) -> Option<u32> {
    if value::is_symbol(symbol_val) {
        Some(encode_symbol_name_id(value::decode_symbol_handle(
            symbol_val,
        )))
    } else {
        None
    }
}

#[cfg(feature = "managed-heap-v2")]
pub(crate) fn canonicalize_v2_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    name_id: u32,
) -> Option<u32> {
    let env = WasmEnv::from_caller(caller)?;
    canonicalize_v2_name_id_with_env(caller, &env, name_id)
}
