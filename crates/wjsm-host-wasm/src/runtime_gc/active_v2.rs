use std::collections::HashSet;
use std::time::Instant;

use super::api::{CycleKind, GcContext, GcStats, RootProvider};
use super::object_walker;
use super::roots::RuntimeRoots;
use crate::WasmEnv;

/// 对 active shared-memory64 heap 执行一次非移动完整回收。
///
/// 顶层与 host side-table roots 复用 `RuntimeRoots`；对象子边由 `HeapAccessV2`
/// 读取同一 8-byte handle table，禁止构造第二份 `ManagedHeap` 或回落 memory32。
pub(crate) fn collect_full<C>(ctx: &mut C, env: &WasmEnv) -> GcStats
where
    C: wasmtime::AsContextMut<Data = crate::RuntimeState>,
{
    let started = Instant::now();
    let algorithm = ctx.as_context().data().gc_algorithm.as_str();
    let mut gc_ctx = GcContext::new(ctx, env, algorithm);
    let handle_count = gc_ctx.obj_table_count();
    let access = gc_ctx.with_state(|state| state.heap_access_v2().clone());

    let mut live = collect_direct_roots(&mut gc_ctx);
    let mut pending = live.iter().copied().collect::<Vec<_>>();
    loop {
        while let Some(handle) = pending.pop() {
            let Ok(references) = access.object_references(handle) else {
                continue;
            };
            for raw in references {
                let mut children = Vec::new();
                object_walker::visit_value_handles(&mut gc_ctx, raw, handle_count, &mut |child| {
                    children.push(child)
                });
                for child in children {
                    if access.resolve_handle(child).is_ok() && live.insert(child) {
                        pending.push(child);
                    }
                }
            }
        }

        let mut added = false;
        for root in collect_host_table_roots(&mut gc_ctx, &live) {
            if access.resolve_handle(root).is_ok() && live.insert(root) {
                pending.push(root);
                added = true;
            }
        }
        if !added {
            break;
        }
    }

    let dead = access
        .live_handles(handle_count as u32)
        .into_iter()
        .filter(|handle| !live.contains(handle))
        .collect::<Vec<_>>();
    let freed_bytes = dead
        .iter()
        .filter_map(|handle| access.retire_handle(*handle).ok())
        .sum::<u64>();

    gc_ctx.with_state(|state| {
        state.reclaim_unmarked_collection_entries(|handle| live.contains(&handle));
        crate::realm::reclaim_dead_realms(state, |handle| live.contains(&handle));
        for handle in &dead {
            crate::array_named_props::ArrayNamedPropsStore::drop_handle(
                &state.array_named_props,
                *handle,
            );
        }
    });
    super::weak_refs::process_weak_refs_after_sweep(&mut gc_ctx, &dead);
    super::weak_refs::cleanup_stream_tables_after_sweep(&mut gc_ctx, &dead);
    gc_ctx.with_state(|state| {
        if let Some(mut free) = state.handle_free_list_for_gc() {
            free.extend_from_slice(&dead);
        }
    });
    if !dead.is_empty() {
        gc_ctx.increment_gc_epoch();
    }

    let mut stats = GcStats {
        marked: live.len(),
        swept: dead.len(),
        freed_bytes: freed_bytes.min(usize::MAX as u64) as usize,
        elapsed: started.elapsed(),
        free_block_count: usize::from(access.free_bytes() != 0),
        total_free_bytes: access.free_bytes().min(usize::MAX as u64) as usize,
        largest_free_block: access.free_bytes().min(usize::MAX as u64) as usize,
        heap_used_bytes: access
            .used_bytes()
            .saturating_sub(access.free_bytes())
            .min(usize::MAX as u64) as usize,
        cycle_kind: CycleKind::Full,
        ..GcStats::default()
    };
    stats.free_bytes_reusable = stats.total_free_bytes;
    stats.ensure_pause_from_elapsed();
    stats
}

fn collect_direct_roots(ctx: &mut GcContext<'_>) -> HashSet<u32> {
    let mut provider = RuntimeRoots;
    let mut roots = HashSet::new();
    provider.for_each_shadow_stack_root(ctx, &mut |handle| {
        roots.insert(handle);
    });
    provider.for_each_wasm_local_root(ctx, &mut |handle| {
        roots.insert(handle);
    });
    roots.retain(|handle| {
        ctx.with_state(|state| state.heap_access_v2().resolve_handle(*handle).is_ok())
    });
    roots
}

fn collect_host_table_roots(ctx: &mut GcContext<'_>, live: &HashSet<u32>) -> Vec<u32> {
    let mut provider = RuntimeRoots;
    let mut roots = Vec::new();
    provider.for_each_host_table_root(ctx, &mut |handle| live.contains(&handle), &mut |handle| {
        roots.push(handle)
    });
    roots
}
