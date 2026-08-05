use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use wasmtime::{AsContext, Caller};
use wjsm_ir::value;

use crate::runtime_string::RuntimeString;
use crate::{RuntimeState, WasmEnv};

// 纯编码/解码从 wjsm-host 再导出，保持 `property_key::*` / `use property_key::*` 路径。
pub(crate) use wjsm_host::{
    DecodedNameId, decode_name_id, encode_runtime_string_name_id, encode_string_name_id,
    encode_symbol_name_id, is_symbol_name_id, name_id_to_property_key_value,
    symbol_value_to_name_id,
};

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

/// 内存 c-string 键 → 规范 name_id 的直接映射缓存（热路径 canonicalize 用）。
///
/// 不用 HashMap：闭包 env 变量访问等热循环每次属性访问都 canonicalize 同一常量键，
/// HashMap 的 SipHash + 分配占可测开销；256 组双路相连命中只需一次取模 + 两次比较，
/// 且无锁（AtomicU64，Relaxed 已足够：同一 index 的 canonicalize 结果幂等，
/// 陈旧读也返回同一 id）。冲突时重新 canonicalize（幂等，安全）。
#[derive(Debug)]
pub(crate) struct MemoryNameIdCache {
    entries: [AtomicU64; MEMORY_NAME_ID_CACHE_WAYS * 2],
}

const MEMORY_NAME_ID_CACHE_WAYS: usize = 256;
const MEMORY_NAME_ID_EMPTY: u64 = u64::MAX;

impl Default for MemoryNameIdCache {
    fn default() -> Self {
        Self {
            entries: std::array::from_fn(|_| AtomicU64::new(MEMORY_NAME_ID_EMPTY)),
        }
    }
}

impl MemoryNameIdCache {
    #[inline]
    pub(crate) fn lookup(&self, index: u32) -> Option<u32> {
        let way0 = (index as usize & (MEMORY_NAME_ID_CACHE_WAYS - 1)) * 2;
        let way1 = way0 + 1;

        for entry in [way0, way1] {
            let packed = self.entries[entry].load(Ordering::Relaxed);
            if packed != MEMORY_NAME_ID_EMPTY && packed >> 32 == u64::from(index) {
                return Some(packed as u32);
            }
        }
        None
    }

    #[inline]
    pub(crate) fn insert(&self, index: u32, id: u32) {
        let way0 = (index as usize & (MEMORY_NAME_ID_CACHE_WAYS - 1)) * 2;
        let way1 = way0 + 1;
        let packed = (u64::from(index) << 32) | u64::from(id);

        if self.entries[way0]
            .compare_exchange(
                MEMORY_NAME_ID_EMPTY,
                packed,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_err()
        {
            self.entries[way1].store(packed, Ordering::Relaxed);
        }
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

pub(crate) fn canonicalize_v2_name_id_with_env<C: AsContext<Data = RuntimeState>>(
    ctx: &C,
    env: &WasmEnv,
    name_id: u32,
) -> Option<u32> {
    match decode_name_id(name_id) {
        DecodedNameId::MemoryString(index) => {
            // 内存 c-string 键一经写入（data segment / bump 分配）内容不再变更，
            // 故可按 (memory index → 规范 name_id) 缓存。闭包 env 变量访问等热循环
            // 每次属性访问都 canonicalize 同一常量键，命中缓存跳过读内存、UTF-8
            // 转换、intern 哈希与临时分配。
            if let Some(cached) = ctx.as_context().data().memory_name_id_cache.lookup(index) {
                return Some(cached);
            }
            let bytes = crate::runtime_render::read_string_bytes_mem(ctx, &env.memory, index);
            let key = RuntimeString::from_utf8_lossy(&bytes);
            let canonical = intern_runtime_property_key(ctx.as_context().data(), key);
            let encoded = encode_runtime_string_name_id(canonical);
            ctx.as_context()
                .data()
                .memory_name_id_cache
                .insert(index, encoded);
            Some(encoded)
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

pub(crate) fn canonicalize_v2_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    name_id: u32,
) -> Option<u32> {
    let env = WasmEnv::from_caller(caller)?;
    canonicalize_v2_name_id_with_env(caller, &env, name_id)
}
