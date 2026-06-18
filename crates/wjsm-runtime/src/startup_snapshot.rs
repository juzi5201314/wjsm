//! Startup snapshot capture / restore.
//!
//! capture: 从 bootstrap 后、用户 JS 前的 wasm memory + RuntimeState 快照 primordial heap。
//! restore: 将快照按当前模块内存布局重定位后写回。

use anyhow::{Context, Result, bail};
use wasmtime::*;

use crate::types::NativeCallable;
use crate::wasm_env::WasmEnv;
use crate::startup_snapshot_format::*;
use crate::{alloc_host_object, alloc_object_with_env};
use wjsm_ir::{constants, value};

use std::sync::{Arc, Mutex};

// ── global read helpers ─────────────────────────────────────────────

fn read_i32_global(store: &mut Store<crate::RuntimeState>, global: Option<Global>) -> i32 {
    global
        .and_then(|g| g.get(&mut *store).i32())
        .unwrap_or(0)
}

fn write_i32_global(store: &mut Store<crate::RuntimeState>, global: Global, val: i32) -> Result<()> {
    global.set(&mut *store, Val::I32(val))?;
    Ok(())
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
    let mut handle_rel_offsets = Vec::with_capacity(obj_table_count as usize);
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
    let runtime_strings: Vec<String> = {
        let strings = store.data().runtime_strings.lock().expect("runtime strings mutex");
        strings.clone()
    };

    // 保存 native_callables
    let native_callables: Vec<SnapshotNativeCallable> = {
        let table = store.data().native_callables.lock().expect("native callables mutex");
        table
            .iter()
            .map(|nc| SnapshotNativeCallable::try_from_native_callable(nc))
            .collect::<Result<Vec<_>>>()
            .context("capture: native_callable not whitelisted")?
    };

    // 检查排除项: 不能让运行态进入 snapshot
    assert_excluded_tables_clean(store)?;

    let async_iterator_prototype = store.data().async_iterator_prototype;
    let async_gen_prototype = store.data().async_gen_prototype;
    let array_proto_values = store.data().array_proto_values.load(std::sync::atomic::Ordering::Relaxed);

    let header = StartupSnapshotHeader {
        magic: SNAPSHOT_MAGIC,
        format_version: SNAPSHOT_FORMAT_VERSION,
        abi_hash: abi_hash(),
        heap_used,
        obj_table_count: obj_table_count as u32,
        function_props_base: function_props_base as u32,
        object_proto_handle,
        array_proto_handle,
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
    })
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
        let t = data.timers.lock().expect("timers");
        let c = data.cancelled_timers.lock().expect("ctimers");
        if !t.is_empty() { bail!("capture: timers not empty ({} entries)", t.len()); }
        if !c.is_empty() { bail!("capture: cancelled_timers not empty"); }
    }
    check_empty!(*data.microtask_queue.lock().expect("m"), "microtask_queue");
    check_empty!(*data.promise_table.lock().expect("p"), "promise_table");
    check_empty!(*data.continuation_table.lock().expect("c"), "continuation_table");
    check_empty!(*data.async_generator_table.lock().expect("ag"), "async_generator_table");
    check_empty!(*data.error_table.lock().expect("e"), "error_table");
    check_empty!(*data.map_table.lock().expect("map"), "map_table");
    check_empty!(*data.set_table.lock().expect("set"), "set_table");
    check_empty!(*data.weakmap_table.lock().expect("wm"), "weakmap_table");
    check_empty!(*data.weakset_table.lock().expect("ws"), "weakset_table");
    check_empty!(*data.weakref_table.lock().expect("wr"), "weakref_table");
    check_empty!(*data.finalization_registry_table.lock().expect("fr"), "finalization_registry");
    check_empty!(*data.proxy_table.lock().expect("px"), "proxy_table");
    check_empty!(*data.arraybuffer_table.lock().expect("ab"), "arraybuffer_table");
    check_empty!(*data.dataview_table.lock().expect("dv"), "dataview_table");
    check_empty!(*data.typedarray_table.lock().expect("ta"), "typedarray_table");
    check_empty!(*data.headers_table.lock().expect("hdr"), "headers_table");
    check_empty!(*data.fetch_response_table.lock().expect("fr"), "fetch_response_table");
    check_empty!(*data.fetch_request_table.lock().expect("frq"), "fetch_request_table");
    check_empty!(*data.abort_signal_table.lock().expect("as"), "abort_signal_table");
    check_empty!(*data.http_response_table.lock().expect("http"), "http_response_table");
    check_empty!(*data.readable_stream_table.lock().expect("rs"), "readable_stream_table");
    check_empty!(*data.reader_table.lock().expect("rdr"), "reader_table");
    check_empty!(*data.stream_controller_table.lock().expect("ctrl"), "stream_controller_table");
    check_empty!(*data.byob_request_table.lock().expect("byob"), "byob_request_table");
    check_empty!(*data.writable_stream_table.lock().expect("ws"), "writable_stream_table");
    check_empty!(*data.writer_table.lock().expect("wrt"), "writer_table");
    check_empty!(*data.transform_stream_table.lock().expect("ts"), "transform_stream_table");
    {
        let ec = data.eval_cache.lock().expect("eval_cache");
        if !ec.is_empty() {
            bail!("capture: eval_cache not empty ({} entries)", ec.len());
        }
    }
    {
        let cc = data.combinator_contexts.lock().expect("cc");
        if !cc.is_empty() {
            bail!("capture: combinator_contexts not empty");
        }
    }
    {
        let afs = data.async_from_sync_iterators.lock().expect("afs");
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

    let heap_start = read_i32_global(store, env.object_heap_start) as u32;
    let heap_used = snapshot.header.heap_used;
    let obj_table_count = snapshot.header.obj_table_count;

    // 确保 memory 足够
    let required_bytes = heap_start + heap_used;
    let data = env.memory.data(&*store);
    if data.len() < required_bytes as usize {
        let pages_needed = ((required_bytes as u64 + 65535) / 65536) as u64;
        let current_pages = data.len() as u64 / 65536;
        if pages_needed > current_pages {
            env.memory.grow(&mut *store, pages_needed - current_pages)?;
        }
    }

    // 恢复 object_bytes
    let data = env.memory.data_mut(&mut *store);
    data[heap_start as usize..heap_start as usize + heap_used as usize]
        .copy_from_slice(snapshot.object_bytes);

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
    let _ = env.heap_ptr.set(&mut *store, Val::I32((heap_start + heap_used) as i32));
    let _ = env.obj_table_count.set(&mut *store, Val::I32(obj_table_count as i32));
    if let Some(function_props_base) = env.function_props_base {
        // 设 function_props_base = 当前 obj_table_count（包含所有已恢复对象），
        // 使 __wjsm_init_function_props 从正确位置开始分配函数属性对象。
        function_props_base.set(&mut *store, Val::I32(obj_table_count as i32))?;
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
        *state.runtime_strings.lock().expect("runtime strings") =
            snapshot.runtime_strings.iter().map(|s| s.to_string()).collect();
        let mut ncs = state.native_callables.lock().expect("native");
        ncs.clear();
        for snap_nc in snapshot.native_callables {
            ncs.push(snap_nc.into_native_callable());
        }
        state.async_iterator_prototype = snapshot.header.async_iterator_prototype;
        state.async_gen_prototype = snapshot.header.async_gen_prototype;
        state.array_proto_values.store(snapshot.header.array_proto_values, std::sync::atomic::Ordering::Relaxed);
    }

    Ok(())
}
