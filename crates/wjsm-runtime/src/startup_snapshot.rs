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
use crate::types::NativeCallable;
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

    let data = env.memory.data(&*store);
    if heap_start == 0 || heap_ptr < heap_start {
        bail!("capture: object heap not initialized");
    }

    // 保存 object_bytes
    let heap_used = (heap_ptr - heap_start) as u32;
    let object_bytes = data[heap_start..heap_ptr].to_vec();

    // 保存 handle rel_offsets: obj_table[0..obj_table_count]
    // 区分 null sentinel（entry == 0，GC sweep 释放槽位）和 rel == 0（entry 恰为
    // heap_start，handle 0 哨兵对象就在此处）：null 槽编码为 NULL_HANDLE_REL，
    // 否则编码为 entry - heap_start。
    let mut handle_rel_offsets = Vec::with_capacity(obj_table_count);
    for i in 0..obj_table_count {
        let offset = obj_table_base + i * 4;
        if offset + 4 > data.len() {
            bail!("capture: obj_table out of bounds at index {i}");
        }
        let entry = u32::from_le_bytes(data[offset..offset + 4].try_into()?);
        if entry == 0 {
            handle_rel_offsets.push(NULL_HANDLE_REL);
        } else if entry < heap_start as u32 || entry >= heap_ptr as u32 {
            bail!(
                "capture: obj_table[{}] = {} not in object heap [{}..{})",
                i,
                entry,
                heap_start,
                heap_ptr
            );
        } else {
            handle_rel_offsets.push(entry - heap_start as u32);
        }
    }

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
        abi_hash: abi_hash(),
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
    let ot_end = ot_base.saturating_add(obj_table_count as usize * 4);
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
    Ok(())
}

// ── restore ─────────────────────────────────────────────────────────

pub(crate) fn restore_startup_snapshot(
    store: &mut Store<crate::RuntimeState>,
    env: &WasmEnv,
    snapshot: StartupSnapshotView<'_>,
) -> Result<()> {
    // ABI hash 验证
    let current_abi = abi_hash();
    if snapshot.header.abi_hash != current_abi {
        bail!(
            "restore: ABI hash mismatch: snapshot={:#018x} current={:#018x}",
            snapshot.header.abi_hash,
            current_abi
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

    let obj_table_base = env.obj_table_ptr.get(&mut *store).i32().unwrap_or(0) as u32;
    let table_end = obj_table_base
        .checked_add(obj_table_count.saturating_mul(4))
        .ok_or_else(|| anyhow::anyhow!("restore: obj_table range overflow"))?;
    let mem_len = env.memory.data(&*store).len() as u32;
    if table_end > mem_len {
        bail!(
            "restore: obj_table [{}..{}) exceeds memory size {}",
            obj_table_base,
            table_end,
            mem_len
        );
    }

    let required_bytes = heap_start
        .checked_add(heap_used)
        .ok_or_else(|| anyhow::anyhow!("restore: heap range overflow"))?;
    if required_bytes as usize > mem_len as usize {
        let pages_needed = (required_bytes as u64).div_ceil(65536);
        let current_pages = mem_len as u64 / 65536;
        if pages_needed > current_pages {
            env.memory
                .grow(&mut *store, pages_needed - current_pages)
                .map_err(|e| anyhow::anyhow!("restore: memory.grow: {e:?}"))?;
        }
    }

    // 恢复 object_bytes，并把 seed 模块内的 Array.prototype 方法表索引重定位到当前模块。
    let data = env.memory.data_mut(&mut *store);
    let heap_start_usize = heap_start as usize;
    let heap_end = heap_start_usize + heap_used as usize;
    data[heap_start_usize..heap_end].copy_from_slice(snapshot.object_bytes);
    remap_array_proto_function_indices(
        &mut data[heap_start_usize..heap_end],
        snapshot.header.arr_proto_table_base,
        snapshot.header.arr_proto_table_len,
        current_arr_proto_table_base,
    )?;

    // 恢复 handle table
    let obj_table_base = env.obj_table_ptr.get(&mut *store).i32().unwrap_or(0) as usize;
    let data = env.memory.data_mut(&mut *store);
    for i in 0..obj_table_count as usize {
        let offset = obj_table_base + i * 4;
        let rel = snapshot.handle_rel_offsets[i];
        // NULL_HANDLE_REL 表示原槽位为 0（GC sweep 释放或显式 null），其它值是
        // 相对 heap_start 的偏移；rel == 0 是合法情况（handle 0 哨兵对象就在
        // heap_start 处），不能与 null 混淆。
        let abs = if rel == NULL_HANDLE_REL {
            0
        } else {
            heap_start + rel
        };
        data[offset..offset + 4].copy_from_slice(&abs.to_le_bytes());
    }

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
