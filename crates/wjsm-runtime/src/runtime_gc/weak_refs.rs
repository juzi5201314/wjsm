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
                if entry.target_handle.is_some_and(|handle| freed.contains(&handle)) {
                    entry.target_handle = None;
                }
            }
        }
        {
            let mut table = st
                .weakmap_table
                .lock()
                .expect("weakmap_table mutex");
            for entry in table.iter_mut() {
                entry.map.retain(|key, _| !freed.contains(key));
            }
        }
        {
            let mut table = st
                .weakset_table
                .lock()
                .expect("weakset_table mutex");
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
            let mut queue = st
                .microtask_queue
                .lock()
                .expect("microtask queue mutex");
            for (callback, held_value) in cleanup_tasks {
                queue.push_back(Microtask::CleanupFinalizationRegistry {
                    callback,
                    held_value,
                });
            }
        }
    });
}