//! Host 侧运行时字符串稳定 handle 表与清扫。
//!
//! runtime string handle 是表索引，不属于 `ManagedHeap` 对象图。表项不移动；清扫仅将
//! 已证明不可达的占用槽改为 `None`，后续分配才可复用该索引。

use std::collections::HashSet;
use std::sync::Mutex;

use anyhow::{Result, bail};

use crate::RuntimeState;
use crate::runtime_gc::GcContext;
use crate::runtime_gc::object_walker::visit_value_references;
use crate::runtime_gc::roots::{collect_host_table_values, collect_reachable_object_handles};
use crate::runtime_string::RuntimeString;

/// 字符串表清扫阈值（估算字节）。
pub(crate) const SWEEP_THRESHOLD_BYTES: usize = 16 * 1024 * 1024;

/// 每项固定开销估算（Vec header + 分配器元数据）。
const PER_ENTRY_OVERHEAD: usize = 64;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RuntimeStringSweepStats {
    pub(crate) reclaimed_entries: usize,
    pub(crate) live_entries: usize,
    pub(crate) live_bytes: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RuntimeStringSweepOutcome {
    Deferred,
    Swept(RuntimeStringSweepStats),
}

struct RuntimeStringTableInner {
    entries: Vec<Option<RuntimeString>>,
    free_slots: Vec<u32>,
    allocated_bytes: usize,
    next_sweep_bytes: usize,
}

impl Default for RuntimeStringTableInner {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            free_slots: Vec::new(),
            allocated_bytes: 0,
            next_sweep_bytes: SWEEP_THRESHOLD_BYTES,
        }
    }
}

#[derive(Default)]
pub(crate) struct RuntimeStringTable {
    inner: Mutex<RuntimeStringTableInner>,
}

impl RuntimeStringTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn alloc(&self, string: RuntimeString) -> u32 {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.allocated_bytes = inner.allocated_bytes.saturating_add(entry_bytes(&string));
        if let Some(slot) = inner.free_slots.pop() {
            let entry = inner
                .entries
                .get_mut(usize::try_from(slot).expect("u32 fits usize"))
                .expect("runtime string free slot must exist");
            assert!(entry.is_none(), "runtime string free slot must be vacant");
            *entry = Some(string);
            return slot;
        }
        let handle = u32::try_from(inner.entries.len()).expect("runtime string handle overflow");
        inner.entries.push(Some(string));
        handle
    }

    pub(crate) fn alloc_many<I>(&self, strings: I) -> Vec<u32>
    where
        I: IntoIterator<Item = RuntimeString>,
    {
        strings
            .into_iter()
            .map(|string| self.alloc(string))
            .collect()
    }

    pub(crate) fn get(&self, handle: u32) -> Option<RuntimeString> {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner
            .entries
            .get(usize::try_from(handle).expect("u32 fits usize"))?
            .clone()
    }

    pub(crate) fn with<R>(&self, handle: u32, read: impl FnOnce(&RuntimeString) -> R) -> Option<R> {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner
            .entries
            .get(usize::try_from(handle).expect("u32 fits usize"))?
            .as_ref()
            .map(read)
    }

    pub(crate) fn needs_sweep(&self) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.allocated_bytes >= inner.next_sweep_bytes
    }

    pub(crate) fn sweep(&self, live_handles: &HashSet<u32>) -> RuntimeStringSweepStats {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let mut stats = RuntimeStringSweepStats::default();
        let mut reclaimed = Vec::new();
        for (index, entry) in inner.entries.iter_mut().enumerate() {
            let Some(string) = entry.as_ref() else {
                continue;
            };
            let handle = u32::try_from(index).expect("runtime string handle overflow");
            if live_handles.contains(&handle) {
                stats.live_entries += 1;
                stats.live_bytes = stats.live_bytes.saturating_add(entry_bytes(string));
            } else {
                entry.take();
                reclaimed.push(handle);
                stats.reclaimed_entries += 1;
            }
        }
        inner.free_slots.extend(reclaimed);
        inner.allocated_bytes = stats.live_bytes;
        inner.next_sweep_bytes = stats
            .live_bytes
            .saturating_mul(2)
            .max(SWEEP_THRESHOLD_BYTES);
        stats
    }

    pub(crate) fn clear(&self) {
        *self.inner.lock().unwrap_or_else(|error| error.into_inner()) =
            RuntimeStringTableInner::default();
    }

    pub(crate) fn snapshot_dense(&self) -> Result<Vec<RuntimeString>> {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let mut strings = Vec::with_capacity(inner.entries.len());
        for (index, entry) in inner.entries.iter().enumerate() {
            let Some(string) = entry else {
                bail!("runtime string snapshot contains vacant slot {index}");
            };
            strings.push(string.clone());
        }
        Ok(strings)
    }

    pub(crate) fn restore_dense<I>(&self, strings: I)
    where
        I: IntoIterator<Item = RuntimeString>,
    {
        let entries: Vec<Option<RuntimeString>> = strings.into_iter().map(Some).collect();
        let allocated_bytes = entries.iter().fold(0usize, |total, entry| {
            total.saturating_add(entry.as_ref().map_or(0, entry_bytes))
        });
        let next_sweep_bytes = allocated_bytes.saturating_mul(2).max(SWEEP_THRESHOLD_BYTES);
        *self.inner.lock().unwrap_or_else(|error| error.into_inner()) = RuntimeStringTableInner {
            entries,
            free_slots: Vec::new(),
            allocated_bytes,
            next_sweep_bytes,
        };
    }
}

fn entry_bytes(string: &RuntimeString) -> usize {
    string
        .utf16_len()
        .saturating_mul(2)
        .saturating_add(PER_ENTRY_OVERHEAD)
}

pub(crate) fn maybe_sweep_runtime_strings<C>(
    ctx: &mut C,
    env: &crate::wasm_env::WasmEnv,
) -> RuntimeStringSweepOutcome
where
    C: wasmtime::AsContextMut<Data = RuntimeState>,
{
    if ctx.as_context_mut().data().runtime_strings.needs_sweep() {
        sweep_runtime_strings(ctx, env)
    } else {
        RuntimeStringSweepOutcome::Swept(RuntimeStringSweepStats::default())
    }
}

pub(crate) fn force_sweep_runtime_strings<C>(
    ctx: &mut C,
    env: &crate::wasm_env::WasmEnv,
) -> RuntimeStringSweepOutcome
where
    C: wasmtime::AsContextMut<Data = RuntimeState>,
{
    sweep_runtime_strings(ctx, env)
}

fn collect_runtime_string_references(
    gc_ctx: &mut GcContext<'_>,
    value: i64,
    obj_table_count: usize,
    live: &mut HashSet<u32>,
) {
    visit_value_references(gc_ctx, value, obj_table_count, &mut |_| {}, &mut |handle| {
        live.insert(handle);
    });
}

fn sweep_runtime_strings<C>(
    ctx: &mut C,
    env: &crate::wasm_env::WasmEnv,
) -> RuntimeStringSweepOutcome
where
    C: wasmtime::AsContextMut<Data = RuntimeState>,
{
    let mut gc_ctx = GcContext::new(ctx, env, "string-sweep");
    let inspector_values = gc_ctx.with_state(|state| match state.inspector.as_ref() {
        Some(inspector) => inspector.try_held_values(),
        None => Some(Vec::new()),
    });
    let Some(inspector_values) = inspector_values else {
        return RuntimeStringSweepOutcome::Deferred;
    };

    let obj_table_count = gc_ctx.obj_table_count();
    let mut live = HashSet::new();
    let sp = gc_ctx.shadow_sp();
    let shadow_values = gc_ctx.with_shadow_memory(|data| {
        data[..sp.min(data.len())]
            .chunks_exact(8)
            .map(|bytes| i64::from_le_bytes(bytes.try_into().expect("eight-byte chunk")))
            .collect::<Vec<_>>()
    });
    for value in shadow_values {
        collect_runtime_string_references(&mut gc_ctx, value, obj_table_count, &mut live);
    }

    let reachable_objects = collect_reachable_object_handles(&mut gc_ctx);
    let access = gc_ctx.with_state(|state| state.heap_access_v2().clone());
    for handle in reachable_objects {
        if let Ok(references) = access.object_references(handle) {
            for value in references {
                collect_runtime_string_references(&mut gc_ctx, value, obj_table_count, &mut live);
            }
        }
    }

    let mut host_values = collect_host_table_values(&mut gc_ctx, &mut |_| true);
    host_values.extend(inspector_values);
    for value in host_values {
        collect_runtime_string_references(&mut gc_ctx, value, obj_table_count, &mut live);
    }

    let stats = gc_ctx.with_state(|state| state.runtime_strings.sweep(&live));
    RuntimeStringSweepOutcome::Swept(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(value: &str) -> RuntimeString {
        RuntimeString::from_utf8_str(value)
    }

    #[test]
    fn runtime_string_table_reclaims_empty_and_non_empty_slots_once() {
        let table = RuntimeStringTable::new();
        let empty = table.alloc(string(""));
        let text = table.alloc(string("text"));

        let first = table.sweep(&HashSet::new());
        let second = table.sweep(&HashSet::new());

        assert_eq!(first.reclaimed_entries, 2);
        assert_eq!(second.reclaimed_entries, 0);
        assert_eq!(table.get(empty), None);
        assert_eq!(table.get(text), None);
        let inner = table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(inner.free_slots.len(), 2);
    }

    #[test]
    fn runtime_string_table_reuses_only_vacant_stable_indices() {
        let table = RuntimeStringTable::new();
        let live = table.alloc(string("live"));
        let dead = table.alloc(string("dead"));
        table.sweep(&HashSet::from([live]));

        let reused = table.alloc(string("replacement"));

        assert_eq!(reused, dead);
        assert_eq!(table.get(live), Some(string("live")));
        assert_eq!(table.get(reused), Some(string("replacement")));
    }

    #[test]
    fn runtime_string_table_snapshot_rejects_vacant_slots() {
        let table = RuntimeStringTable::new();
        table.alloc(string("dead"));
        table.sweep(&HashSet::new());

        let error = table
            .snapshot_dense()
            .expect_err("vacant table must not be snapshotted");

        assert!(error.to_string().contains("vacant slot 0"));
    }

    #[test]
    fn runtime_string_table_restore_rebuilds_bytes_and_threshold() {
        let table = RuntimeStringTable::new();
        table.restore_dense([string(""), string("abc")]);

        let inner = table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let expected = entry_bytes(&string("")) + entry_bytes(&string("abc"));
        assert_eq!(inner.allocated_bytes, expected);
        assert_eq!(
            inner.next_sweep_bytes,
            expected.saturating_mul(2).max(SWEEP_THRESHOLD_BYTES)
        );
        assert!(inner.free_slots.is_empty());
    }
}
