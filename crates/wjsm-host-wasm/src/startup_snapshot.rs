//! Startup snapshot capture / restore.
//!
//! capture: 从 bootstrap 后、用户 JS 前的 wasm memory + RuntimeState 快照 primordial heap。
//! restore: 将快照按当前模块内存布局重定位后写回。

use anyhow::{Context, Result, bail};
use wasmtime::*;
use wjsm_ir::value;

use crate::runtime_string::RuntimeString;
use crate::startup_snapshot_native_bridge::SnapshotNativeCallableBridge;
use crate::startup_snapshot_remap::remap_array_proto_function_indices;
use crate::types::{NativeCallable, TypedArrayConstructorKind};
use crate::wasm_env::WasmEnv;
use wjsm_snapshot_format::*;

// ── global read helpers ─────────────────────────────────────────────

fn read_i32_global(store: &mut Store<crate::RuntimeState>, global: Option<Global>) -> i32 {
    global.and_then(|g| g.get(&mut *store).i32()).unwrap_or(0)
}

fn read_required_u32_global(
    store: &mut Store<crate::RuntimeState>,
    global: Option<Global>,
    name: &str,
) -> Result<u32> {
    let global = global.ok_or_else(|| anyhow::anyhow!("{name} export missing"))?;
    let value = global
        .get(&mut *store)
        .i32()
        .ok_or_else(|| anyhow::anyhow!("{name} export is not i32"))?;
    if value < 0 {
        bail!("{name} export is negative: {value}");
    }
    Ok(value as u32)
}

fn read_required_u64_global(
    store: &mut Store<crate::RuntimeState>,
    global: Option<Global>,
    name: &str,
) -> Result<u64> {
    let global = global.ok_or_else(|| anyhow::anyhow!("{name} export missing"))?;
    Ok(global
        .get(&mut *store)
        .i64()
        .ok_or_else(|| anyhow::anyhow!("{name} export is not i64"))? as u64)
}

// ── capture ─────────────────────────────────────────────────────────

pub(crate) fn capture_startup_snapshot(
    store: &mut Store<crate::RuntimeState>,
    env: &WasmEnv,
    snapshot_abi_hash: u64,
) -> Result<StartupSnapshotOwned> {
    let heap_start = read_i32_global(store, env.object_heap_start) as usize;
    let heap_ptr = env.heap_ptr.get(&mut *store).i32().unwrap_or(0) as usize;
    let obj_table_base = env.obj_table_ptr.get(&mut *store).i32().unwrap_or(0) as usize;
    let obj_table_count = env.obj_table_count.get(&mut *store).i32().unwrap_or(0) as usize;
    let function_props_base = read_i32_global(store, env.function_props_base) as usize;
    let object_proto_handle = env.object_proto_handle.get(&mut *store).i32().unwrap_or(-1) as u32;
    let array_proto_handle = env.array_proto_handle.get(&mut *store).i32().unwrap_or(-1) as u32;
    let arr_proto_table_base =
        read_required_u32_global(store, env.arr_proto_table_base, "__arr_proto_table_base")?;
    let arr_proto_table_len =
        read_required_u32_global(store, env.arr_proto_table_len, "__arr_proto_table_len")?;
    let arr_proto_table_hash =
        read_required_u64_global(store, env.arr_proto_table_hash, "__arr_proto_table_hash")?;

    // V2：对象体与 handle table 均在 memory64 ManagedHeap。
    let access = store.data().heap_access_v2().clone();
    let object_base = access.object_heap_base();
    let object_bytes = access
        .capture_object_region()
        .map_err(|e| anyhow::anyhow!("capture: V2 object region: {e}"))?;
    let heap_used = u32::try_from(object_bytes.len())
        .map_err(|_| anyhow::anyhow!("capture: V2 object region exceeds u32"))?;
    if heap_used == 0 && obj_table_count == 0 {
        bail!("capture: V2 object heap not initialized");
    }

    let mut handle_rel_offsets = Vec::with_capacity(obj_table_count);
    for i in 0..obj_table_count {
        match access.resolve_handle(i as u32) {
            Ok(addr) => {
                if addr < object_base {
                    bail!("capture: V2 handle {i} address {addr:#x} below object base");
                }
                let rel = addr - object_base;
                if rel >= u64::from(heap_used) {
                    bail!(
                        "capture: V2 handle {i} rel {rel} outside object region (heap_used={heap_used})"
                    );
                }
                if rel > u32::MAX as u64 {
                    bail!("capture: V2 handle {i} rel {rel} exceeds u32");
                }
                handle_rel_offsets.push(rel as u32);
            }
            Err(_) => handle_rel_offsets.push(NULL_HANDLE_REL),
        }
    }
    let _ = (heap_start, heap_ptr, obj_table_base);

    // 保存 runtime_strings
    let runtime_strings: Vec<SnapshotRuntimeString> = {
        let strings = store
            .data()
            .runtime_strings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        strings
            .iter()
            .map(|s| s.as_utf16_units().to_vec())
            .collect()
    };

    // 保存 native_callables
    let (native_callables, native_callable_methods): (Vec<SnapshotNativeCallable>, Vec<u8>) = {
        let table = store
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut ncs = Vec::with_capacity(table.len());
        let mut methods = Vec::with_capacity(table.len());
        for nc in table.iter() {
            ncs.push(
                SnapshotNativeCallable::try_from_native_callable(nc)
                    .context("capture: native_callable not whitelisted")?,
            );
            methods.push(match nc {
                NativeCallable::NumberPrimitiveMethod { method }
                | NativeCallable::BigIntPrimitiveMethod { method }
                | NativeCallable::SymbolPrimitiveMethod { method }
                | NativeCallable::RegExpPrimitiveMethod { method } => *method,
                NativeCallable::TypedArrayConstructor(kind) => kind.index() as u8,
                NativeCallable::OsInfo { kind } => kind.method(),
                NativeCallable::FsMethod { kind } => kind.method(),
                NativeCallable::ZlibMethod { kind } => kind.method(),
                NativeCallable::ChildProcessMethod { kind } => kind.method(),
                NativeCallable::NetMethod { kind } => kind.method(),
                NativeCallable::VmMethod { kind } => kind.method(),
                NativeCallable::DgramMethod { kind } => kind.method(),
                NativeCallable::TlsMethod { kind } => kind.method(),
                NativeCallable::WorkerThreadsMethod { kind } => kind.method(),
                NativeCallable::PerfHooksMethod { kind } => kind.method(),
                _ => 0,
            });
        }
        (ncs, methods)
    };

    // 检查排除项: 不能让运行态进入 snapshot
    assert_excluded_tables_clean(store)?;

    let iterator_prototype = store.data().iterator_prototype;
    let generator_prototype = store.data().generator_prototype;
    let async_iterator_prototype = store.data().async_iterator_prototype;
    let async_gen_prototype = store.data().async_gen_prototype;
    let array_proto_values = store
        .data()
        .array_proto_values
        .load(std::sync::atomic::Ordering::Relaxed);

    let header = StartupSnapshotHeader {
        magic: SNAPSHOT_MAGIC,
        format_version: SNAPSHOT_FORMAT_VERSION,
        abi_hash: snapshot_abi_hash,
        heap_used,
        immortal_objects_end_rel: heap_used,
        obj_table_count: obj_table_count as u32,
        function_props_base: function_props_base as u32,
        object_proto_handle,
        array_proto_handle,
        arr_proto_table_base,
        arr_proto_table_len,
        arr_proto_table_hash,
        iterator_prototype,
        generator_prototype,
        async_iterator_prototype,
        async_gen_prototype,
        array_proto_values,
    };

    Ok(StartupSnapshotOwned {
        header,
        object_bytes,
        handle_rel_offsets,
        runtime_strings,
        native_callables,
        native_callable_methods,
    })
}

/// Clear primordial heap / handle table / host bootstrap side state before snapshot restore
/// when shared memory still holds objects from an earlier cold bootstrap (issue #113).
pub(crate) fn reset_primordial_heap_before_restore(
    store: &mut Store<crate::RuntimeState>,
    env: &WasmEnv,
) -> Result<()> {
    let heap_start = read_i32_global(store, env.object_heap_start) as u32;
    let heap_ptr = env.heap_ptr.get(&mut *store).i32().unwrap_or(0) as u32;
    let obj_table_base = env.obj_table_ptr.get(&mut *store).i32().unwrap_or(0) as u32;
    let obj_table_count = env.obj_table_count.get(&mut *store).i32().unwrap_or(0) as u32;
    if heap_ptr <= heap_start && obj_table_count == 0 {
        return Ok(());
    }

    let mem = env.memory.data_mut(&mut *store);
    let hs = heap_start as usize;
    let hp = heap_ptr as usize;
    if hp > hs && hp <= mem.len() {
        mem[hs..hp].fill(0);
    }
    let ot_base = obj_table_base as usize;
    let ot_end = ot_base.saturating_add(
        obj_table_count as usize * wjsm_ir::constants::HANDLE_TABLE_ENTRY_SIZE as usize,
    );
    if ot_end <= mem.len() {
        mem[ot_base..ot_end].fill(0);
    }

    let _ = env.heap_ptr.set(&mut *store, Val::I32(heap_start as i32));
    let _ = env.obj_table_count.set(&mut *store, Val::I32(0));
    if let Some(bootstrap_done) = env.bootstrap_done {
        bootstrap_done.set(&mut *store, Val::I32(0))?;
    }
    if let Some(function_props_done) = env.function_props_done {
        function_props_done.set(&mut *store, Val::I32(0))?;
    }
    let _ = env.array_proto_handle.set(&mut *store, Val::I32(-1));
    let _ = env.object_proto_handle.set(&mut *store, Val::I32(-1));

    let state = store.data_mut();
    state
        .runtime_strings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    state
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    crate::symbol_well_known::clear_symbol_constructor_static_props(state);
    state.iterator_prototype = value::encode_undefined();
    state.generator_prototype = value::encode_undefined();
    state.async_iterator_prototype = value::encode_undefined();
    state.async_gen_prototype = value::encode_undefined();
    state.buffer_prototype = value::encode_undefined();
    state.text_encoder_prototype = value::encode_undefined();
    state.text_decoder_prototype = value::encode_undefined();
    state.typedarray_prototypes = [value::encode_undefined(); TypedArrayConstructorKind::COUNT];
    state.array_proto_values.store(
        value::encode_undefined(),
        std::sync::atomic::Ordering::Relaxed,
    );
    Ok(())
}

fn assert_excluded_tables_clean(store: &Store<crate::RuntimeState>) -> Result<()> {
    let data = store.data();
    macro_rules! check_empty {
        ($lock:expr, $name:expr) => {
            if !$lock.is_empty() {
                bail!("capture: {} not empty ({} entries)", $name, $lock.len());
            }
        };
    }
    {
        let t = data.timers.lock().unwrap_or_else(|e| e.into_inner());
        let c = data
            .cancelled_timers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !t.is_empty() {
            bail!("capture: timers not empty ({} entries)", t.len());
        }
        if !c.is_empty() {
            bail!("capture: cancelled_timers not empty");
        }
    }
    check_empty!(
        *data
            .microtask_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        "microtask_queue"
    );
    check_empty!(
        *data.promise_table.lock().unwrap_or_else(|e| e.into_inner()),
        "promise_table"
    );
    check_empty!(
        *data
            .continuation_table
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        "continuation_table"
    );
    check_empty!(
        *data
            .async_generator_table
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        "async_generator_table"
    );
    check_empty!(
        *data.error_table.lock().unwrap_or_else(|e| e.into_inner()),
        "error_table"
    );
    check_empty!(
        *data.map_table.lock().unwrap_or_else(|e| e.into_inner()),
        "map_table"
    );
    check_empty!(
        *data.set_table.lock().unwrap_or_else(|e| e.into_inner()),
        "set_table"
    );
    check_empty!(
        *data.weakmap_table.lock().unwrap_or_else(|e| e.into_inner()),
        "weakmap_table"
    );
    check_empty!(
        *data.weakset_table.lock().unwrap_or_else(|e| e.into_inner()),
        "weakset_table"
    );
    check_empty!(
        *data.weakref_table.lock().unwrap_or_else(|e| e.into_inner()),
        "weakref_table"
    );
    check_empty!(
        *data
            .finalization_registry_table
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        "finalization_registry"
    );
    check_empty!(
        *data.proxy_table.lock().unwrap_or_else(|e| e.into_inner()),
        "proxy_table"
    );
    check_empty!(
        *data
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        "arraybuffer_table"
    );
    check_empty!(
        *data
            .dataview_table
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        "dataview_table"
    );
    check_empty!(
        *data
            .typedarray_table
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        "typedarray_table"
    );
    check_empty!(
        *data.headers_table.lock().unwrap_or_else(|e| e.into_inner()),
        "headers_table"
    );
    check_empty!(
        *data
            .fetch_response_table
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        "fetch_response_table"
    );
    check_empty!(
        *data
            .fetch_request_table
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        "fetch_request_table"
    );
    check_empty!(
        *data
            .abort_signal_table
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        "abort_signal_table"
    );
    check_empty!(
        *data
            .http_response_table
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        "http_response_table"
    );
    if !data.readable_stream_table.is_empty() {
        bail!("capture: readable_stream_table not empty");
    }
    if !data.reader_table.is_empty() {
        bail!("capture: reader_table not empty");
    }
    if !data.stream_controller_table.is_empty() {
        bail!("capture: stream_controller_table not empty");
    }
    if !data.byob_request_table.is_empty() {
        bail!("capture: byob_request_table not empty");
    }
    if !data.writable_stream_table.is_empty() {
        bail!("capture: writable_stream_table not empty");
    }
    if !data.writer_table.is_empty() {
        bail!("capture: writer_table not empty");
    }
    if !data.transform_stream_table.is_empty() {
        bail!("capture: transform_stream_table not empty");
    }
    {
        let ec = data.eval_cache.lock().unwrap_or_else(|e| e.into_inner());
        if !ec.is_empty() {
            bail!("capture: eval_cache not empty ({} entries)", ec.len());
        }
    }
    {
        let cc = data
            .combinator_contexts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !cc.is_empty() {
            bail!("capture: combinator_contexts not empty");
        }
    }
    {
        let afs = data
            .async_from_sync_iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !afs.is_empty() {
            bail!("capture: async_from_sync_iterators not empty");
        }
    }
    if !data
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty_for_snapshot()
    {
        bail!("capture: async_hooks runtime state not empty");
    }
    let histogram_registry_empty = data
        .shared_state
        .as_ref()
        .map(|shared| shared.perf_histograms.is_empty())
        .transpose()
        .map_err(|error| anyhow::anyhow!("capture: perf_hooks histogram registry: {error}"))?
        .unwrap_or(true);
    if data
        .performance_observer_mask
        .load(std::sync::atomic::Ordering::Relaxed)
        != 0
        || !value::is_undefined(
            data.performance_native_sink
                .load(std::sync::atomic::Ordering::Relaxed),
        )
        || !value::is_undefined(
            data.performance_native_converter
                .load(std::sync::atomic::Ordering::Relaxed),
        )
        || !value::is_undefined(
            data.performance_native_dispatcher
                .load(std::sync::atomic::Ordering::Relaxed),
        )
        || !data
            .performance_native_entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty()
        || !data
            .performance_event_loop_monitors
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty()
        || data
            .performance_native_delivery_scheduled
            .load(std::sync::atomic::Ordering::Relaxed)
        || data
            .performance_forced_gc
            .load(std::sync::atomic::Ordering::Relaxed)
        || !value::is_undefined(
            data.performance_histogram_base_prototype
                .load(std::sync::atomic::Ordering::Relaxed),
        )
        || !value::is_undefined(
            data.performance_histogram_recordable_prototype
                .load(std::sync::atomic::Ordering::Relaxed),
        )
        || !value::is_undefined(
            data.performance_histogram_interval_prototype
                .load(std::sync::atomic::Ordering::Relaxed),
        )
        || !data.performance_histogram_wrappers.is_empty()
        || !histogram_registry_empty
    {
        bail!("capture: perf_hooks runtime state not empty");
    }
    Ok(())
}

// ── restore ─────────────────────────────────────────────────────────

pub(crate) fn restore_startup_snapshot(
    store: &mut Store<crate::RuntimeState>,
    env: &WasmEnv,
    snapshot: StartupSnapshotView<'_>,
    expected_abi_hash: u64,
) -> Result<()> {
    // ABI hash 验证
    if snapshot.header.abi_hash != expected_abi_hash {
        bail!(
            "restore: ABI hash mismatch: snapshot={:#018x} current={:#018x}",
            snapshot.header.abi_hash,
            expected_abi_hash
        );
    }

    reset_primordial_heap_before_restore(store, env)?;

    let heap_start = read_i32_global(store, env.object_heap_start) as u32;
    let heap_used = snapshot.header.heap_used;
    let obj_table_count = snapshot.header.obj_table_count;
    let current_arr_proto_table_base =
        read_required_u32_global(store, env.arr_proto_table_base, "__arr_proto_table_base")?;
    let current_arr_proto_table_len =
        read_required_u32_global(store, env.arr_proto_table_len, "__arr_proto_table_len")?;
    let current_arr_proto_table_hash =
        read_required_u64_global(store, env.arr_proto_table_hash, "__arr_proto_table_hash")?;

    if snapshot.header.arr_proto_table_len != current_arr_proto_table_len {
        bail!(
            "restore: Array.prototype table length mismatch: snapshot={} current={}",
            snapshot.header.arr_proto_table_len,
            current_arr_proto_table_len
        );
    }
    if snapshot.header.arr_proto_table_hash != current_arr_proto_table_hash {
        bail!(
            "restore: Array.prototype table hash mismatch: snapshot={:#018x} current={:#018x}",
            snapshot.header.arr_proto_table_hash,
            current_arr_proto_table_hash
        );
    }

    if snapshot.object_bytes.len() != heap_used as usize {
        bail!(
            "restore: object_bytes len {} != heap_used {}",
            snapshot.object_bytes.len(),
            heap_used
        );
    }

    // V2 handle table 在 memory64，无需校验主 memory obj_table 范围。
    let _ = env.obj_table_ptr.get(&mut *store);

    // 恢复 V2 对象区 + handle table。
    let access = store.data().heap_access_v2().clone();
    let mut object_bytes = snapshot.object_bytes.to_vec();
    remap_array_proto_function_indices(
        &mut object_bytes,
        snapshot.header.arr_proto_table_base,
        snapshot.header.arr_proto_table_len,
        current_arr_proto_table_base,
    )?;
    access
        .restore_object_region(&object_bytes)
        .map_err(|e| anyhow::anyhow!("restore: V2 object region: {e}"))?;
    let object_base = access.object_heap_base();
    for i in 0..obj_table_count as usize {
        let rel = snapshot.handle_rel_offsets[i];
        if rel == NULL_HANDLE_REL {
            continue;
        }
        let addr = object_base + u64::from(rel);
        access
            .bind_handle(i as u32, addr)
            .map_err(|e| anyhow::anyhow!("restore: V2 bind handle {i} at {addr:#x}: {e}"))?;
    }
    let _ = heap_start;

    // 写回 globals
    let _ = env
        .heap_ptr
        .set(&mut *store, Val::I32((heap_start + heap_used) as i32));
    let _ = env
        .obj_table_count
        .set(&mut *store, Val::I32(obj_table_count as i32));
    if let Some(function_props_base) = env.function_props_base {
        // 使用 snapshot header 中的 function_props_base（capture 时记录的值），
        // 而非当前 obj_table_count — 保证 primordial 原型与函数属性对象 handle 区间一致。
        function_props_base.set(
            &mut *store,
            Val::I32(snapshot.header.function_props_base as i32),
        )?;
    }
    if let Some(bootstrap_done) = env.bootstrap_done {
        bootstrap_done.set(&mut *store, Val::I32(1))?;
    }
    if let Some(function_props_done) = env.function_props_done {
        function_props_done.set(&mut *store, Val::I32(0))?;
    }
    let _ = env.object_proto_handle.set(
        &mut *store,
        Val::I32(snapshot.header.object_proto_handle as i32),
    );
    let _ = env.array_proto_handle.set(
        &mut *store,
        Val::I32(snapshot.header.array_proto_handle as i32),
    );

    // 重建 RuntimeState
    {
        let state = store.data_mut();
        *state
            .runtime_strings
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = snapshot
            .runtime_strings
            .iter()
            .cloned()
            .map(RuntimeString::from_utf16_units)
            .collect();
        let mut ncs = state
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        ncs.clear();
        for (i, snap_nc) in snapshot.native_callables.iter().enumerate() {
            let method = snapshot
                .native_callable_methods
                .get(i)
                .copied()
                .unwrap_or(0);
            ncs.push(snap_nc.into_native_callable(method));
        }
        state
            .native_callable_free_slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        state.iterator_prototype = snapshot.header.iterator_prototype;
        state.generator_prototype = snapshot.header.generator_prototype;
        state.async_iterator_prototype = snapshot.header.async_iterator_prototype;
        state.async_gen_prototype = snapshot.header.async_gen_prototype;
        state.array_proto_values.store(
            snapshot.header.array_proto_values,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn assert_perf_hooks_state_rejected<T>(mutate: impl FnOnce(&crate::RuntimeState) -> T) {
        let engine = Engine::default();
        let store = Store::new(&engine, crate::RuntimeState::new());
        assert_excluded_tables_clean(&store).expect("fresh runtime state must be snapshot-clean");

        let dirty_state = mutate(store.data());

        let error = assert_excluded_tables_clean(&store)
            .expect_err("perf_hooks runtime state must be excluded from startup snapshots");
        assert!(
            error
                .to_string()
                .contains("perf_hooks runtime state not empty"),
            "unexpected error: {error:#}"
        );
        drop(dirty_state);
    }

    #[test]
    fn snapshot_rejects_perf_hooks_prototypes_registry_and_pending_flags() {
        assert_perf_hooks_state_rejected(|state| {
            state
                .performance_histogram_base_prototype
                .store(value::encode_null(), Ordering::Relaxed);
        });
        assert_perf_hooks_state_rejected(|state| {
            state
                .performance_histogram_recordable_prototype
                .store(value::encode_null(), Ordering::Relaxed);
        });
        assert_perf_hooks_state_rejected(|state| {
            state
                .performance_histogram_interval_prototype
                .store(value::encode_null(), Ordering::Relaxed);
        });
        assert_perf_hooks_state_rejected(|state| {
            state
                .shared_state
                .as_ref()
                .expect("shared runtime state")
                .perf_histograms
                .create(1, 1_000, 3)
                .expect("create histogram")
        });
        assert_perf_hooks_state_rejected(|state| {
            state
                .performance_native_delivery_scheduled
                .store(true, Ordering::Relaxed);
        });
        assert_perf_hooks_state_rejected(|state| {
            state.performance_forced_gc.store(true, Ordering::Relaxed);
        });
    }

    #[test]
    fn snapshot_rejects_perf_hooks_observer_queue_and_monitor_state() {
        assert_perf_hooks_state_rejected(|state| {
            state.performance_observer_mask.store(1, Ordering::Relaxed);
        });
        assert_perf_hooks_state_rejected(|state| {
            state
                .performance_native_sink
                .store(value::encode_null(), Ordering::Relaxed);
        });
        assert_perf_hooks_state_rejected(|state| {
            state
                .performance_observer_mask
                .store(1 << 6, Ordering::Relaxed);
            crate::runtime_node_perf_hooks::queue_resource_entry(
                state,
                crate::runtime_node_perf_hooks::NativeResourceTiming {
                    name: "https://snapshot.invalid/".to_string(),
                    start_time: 1.0,
                    request_start_time: 2.0,
                    response_start_time: 3.0,
                    end_time: 4.0,
                    response_status: 200,
                    encoded_body_size: 5,
                    decoded_body_size: 6,
                },
            );
            state.performance_observer_mask.store(0, Ordering::Relaxed);
        });
        assert_perf_hooks_state_rejected(|state| {
            let capability = state
                .shared_state
                .as_ref()
                .expect("shared runtime state")
                .perf_histograms
                .create(1_000, 1_000_000, 3)
                .expect("create event loop histogram");
            state
                .performance_event_loop_monitors
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    0,
                    crate::runtime_node_perf_hooks::EventLoopDelayMonitor {
                        capability,
                        resolution: std::time::Duration::from_millis(10),
                        next_deadline: None,
                        enabled: false,
                    },
                );
        });
    }

    #[test]
    fn snapshot_rejects_perf_hooks_histogram_wrapper_side_table() {
        assert_perf_hooks_state_rejected(|state| {
            let capability = state
                .shared_state
                .as_ref()
                .expect("shared runtime state")
                .perf_histograms
                .create(1, 1_000, 3)
                .expect("create histogram");
            state.performance_histogram_wrappers.alloc(
                crate::runtime_node_perf_hooks_histogram::HistogramWrapperEntry {
                    capability,
                    kind: 1,
                },
            )
        });
    }
}
