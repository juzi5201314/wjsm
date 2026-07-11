//! sweep 之后处理 WeakRef / FinalizationRegistry / WeakMap / WeakSet 侧表。

use std::collections::HashSet;

use crate::runtime_gc::api::{GcContext, Handle};
use crate::types::Microtask;

/// sweep 清空 obj_table 槽后：清除失效弱引用、修剪 WeakMap/WeakSet、调度 FinalizationRegistry 回调。
pub fn process_weak_refs_after_sweep(ctx: &mut GcContext, freed_handles: &[Handle]) {
    if freed_handles.is_empty() {
        return;
    }
    let freed: HashSet<u32> = freed_handles.iter().copied().collect();
    ctx.with_state(|st| {
        {
            let mut table = st.weakref_table.lock().expect("weakref table mutex");
            for entry in table.iter_mut() {
                if entry
                    .target_handle
                    .is_some_and(|handle| freed.contains(&handle))
                {
                    entry.target_handle = None;
                }
            }
        }
        {
            let mut table = st.weakmap_table.lock().expect("weakmap_table mutex");
            for entry in table.iter_mut() {
                entry.map.retain(|key, _| !freed.contains(key));
            }
        }
        {
            let mut table = st.weakset_table.lock().expect("weakset_table mutex");
            for entry in table.iter_mut() {
                entry.set.retain(|key| !freed.contains(key));
            }
        }
        let mut cleanup_tasks: Vec<(i64, i64)> = Vec::new();
        {
            let mut table = st
                .finalization_registry_table
                .lock()
                .expect("finalization registry table mutex");
            for entry in table.iter_mut() {
                let mut i = 0;
                while i < entry.registrations.len() {
                    if freed.contains(&entry.registrations[i].target_handle) {
                        let reg = entry.registrations.remove(i);
                        cleanup_tasks.push((entry.callback, reg.held_value));
                    } else {
                        i += 1;
                    }
                }
            }
        }
        if !cleanup_tasks.is_empty() {
            let mut queue = st.microtask_queue.lock().expect("microtask queue mutex");
            for (callback, held_value) in cleanup_tasks {
                queue.push_back(Microtask::CleanupFinalizationRegistry {
                    callback,
                    held_value,
                });
            }
        }
    });
}

/// sweep 清空 obj_table 槽后：对 7 张流侧表执行可达性传播 + 回收不可达条目。
pub fn cleanup_stream_tables_after_sweep(ctx: &mut GcContext, freed_handles: &[Handle]) {
    if freed_handles.is_empty() {
        return;
    }
    let freed_set: std::collections::HashSet<u32> = freed_handles.iter().copied().collect();

    ctx.with_state(|st| {
        use std::collections::HashSet;

        // 1. 收集每张表的直接 root（活 wrapper 或 pin）
        let readable_roots = st
            .readable_stream_table
            .direct_roots_after_pruning(&freed_set);
        let reader_roots = st.reader_table.direct_roots_after_pruning(&freed_set);
        let controller_roots = st
            .stream_controller_table
            .direct_roots_after_pruning(&freed_set);
        let byob_roots = st.byob_request_table.direct_roots_after_pruning(&freed_set);
        let writable_roots = st
            .writable_stream_table
            .direct_roots_after_pruning(&freed_set);
        let writer_roots = st.writer_table.direct_roots_after_pruning(&freed_set);
        let transform_roots = st
            .transform_stream_table
            .direct_roots_after_pruning(&freed_set);

        // 2. 传播可达性：stream ↔ controller ↔ byob, reader → stream, writer → writable, transform → readable+writable
        let mut reachable = HashSet::new();
        reachable.extend(readable_roots);
        reachable.extend(reader_roots);
        reachable.extend(controller_roots);
        reachable.extend(byob_roots);
        reachable.extend(writable_roots);
        reachable.extend(writer_roots);
        reachable.extend(transform_roots);

        let mut worklist: Vec<(u8, u32)> = reachable.iter().map(|&h| (0u8, h)).collect();
        let mut visited = reachable.clone();

        while let Some((table_kind, handle)) = worklist.pop() {
            match table_kind {
                0 => {
                    // readable_stream
                    if let Ok(inner) = st.readable_stream_table.inner.lock()
                        && let Some(entry) = inner.get(handle as usize)
                            && let Some(ctrl_h) = entry.controller_handle
                                && visited.insert(ctrl_h) {
                                    worklist.push((2, ctrl_h));
                                }
                }
                1 => {
                    // reader
                    if let Ok(inner) = st.reader_table.inner.lock()
                        && let Some(entry) = inner.get(handle as usize) {
                            let stream_h = entry.stream_handle;
                            if visited.insert(stream_h) {
                                worklist.push((0, stream_h));
                            }
                        }
                }
                2 => {
                    // controller
                    if let Ok(inner) = st.stream_controller_table.inner.lock()
                        && let Some(entry) = inner.get(handle as usize) {
                            let stream_h = entry.stream_handle;
                            if visited.insert(stream_h) {
                                worklist.push((0, stream_h));
                            }
                            if let Some(byob_h) = entry.active_byob_request
                                && visited.insert(byob_h) {
                                    worklist.push((3, byob_h));
                                }
                        }
                }
                3 => {
                    // byob
                    if let Ok(inner) = st.byob_request_table.inner.lock()
                        && let Some(entry) = inner.get(handle as usize) {
                            if visited.insert(entry.controller_handle) {
                                worklist.push((2, entry.controller_handle));
                            }
                            if visited.insert(entry.reader_handle) {
                                worklist.push((1, entry.reader_handle));
                            }
                        }
                }
                4 => {
                    // writable_stream
                    if let Ok(inner) = st.writable_stream_table.inner.lock()
                        && let Some(entry) = inner.get(handle as usize)
                            && let Some(ctrl_h) = entry.controller_handle
                                && visited.insert(ctrl_h) {
                                    worklist.push((2, ctrl_h));
                                }
                }
                5 => {
                    // writer
                    if let Ok(inner) = st.writer_table.inner.lock()
                        && let Some(entry) = inner.get(handle as usize) {
                            let writable_h = entry.writable_stream_handle;
                            if visited.insert(writable_h) {
                                worklist.push((4, writable_h));
                            }
                        }
                }
                6 => {
                    // transform_stream
                    if let Ok(inner) = st.transform_stream_table.inner.lock()
                        && let Some(entry) = inner.get(handle as usize) {
                            if let Some(readable_h) = entry.readable_stream_handle
                                && visited.insert(readable_h) {
                                    worklist.push((0, readable_h));
                                }
                            if let Some(writable_h) = entry.writable_stream_handle
                                && visited.insert(writable_h) {
                                    worklist.push((4, writable_h));
                                }
                        }
                }
                _ => {}
            }
        }

        // 3. 回收不可达条目
        st.readable_stream_table.reclaim_unreachable(&visited);
        st.reader_table.reclaim_unreachable(&visited);
        st.stream_controller_table.reclaim_unreachable(&visited);
        st.byob_request_table.reclaim_unreachable(&visited);
        st.writable_stream_table.reclaim_unreachable(&visited);
        st.writer_table.reclaim_unreachable(&visited);
        st.transform_stream_table.reclaim_unreachable(&visited);
    });
}
