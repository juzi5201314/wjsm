//! Active `--gc zgc` 路径：在唯一 `HeapAccessV2` 上驱动 young/old/director。
//!
//! Young/Old controller 的 phase machine 仍用即时构图；图来自当前 live handle 的
//! `object_references`，禁止第二份 ManagedHeap。非 zgc 算法继续走 [`super::active_v2`]。

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use super::api::{CycleKind, GcContext, GcStats, RootProvider};
use super::control::RootSnapshot;
use super::heap_access_v2::HeapAccessV2;
use super::object_walker;
use super::roots::RuntimeRoots;
use super::zgc::director::{DirectorDecision, GcDirector};
use super::zgc::{OldController, YoungController};
use crate::WasmEnv;
use crate::heap::{HandleGeneration, HandleId};
use super::cpu_time;

/// 按算法名分派 active full collect：仅 `zgc` 走 generational phase machine。
pub(crate) fn collect_dispatch<C>(ctx: &mut C, env: &WasmEnv, algorithm: &str) -> GcStats
where
    C: wasmtime::AsContextMut<Data = crate::RuntimeState>,
{
    if algorithm == "zgc" {
        collect_full(ctx, env)
    } else {
        super::active_v2::collect_full(ctx, env)
    }
}

/// 在 active shared-memory64 heap 上跑一轮 young-led ZGC full collect。
pub(crate) fn collect_full<C>(ctx: &mut C, env: &WasmEnv) -> GcStats
where
    C: wasmtime::AsContextMut<Data = crate::RuntimeState>,
{
    let started = Instant::now();
    let gc_cpu_start = cpu_time::thread_cpu_ns();
    let mut gc_ctx = GcContext::new(ctx, env, "zgc");
    let handle_count = gc_ctx.obj_table_count() as u32;
    let access = gc_ctx.with_state(|state| state.heap_access_v2().clone());

    let mut live_roots = collect_direct_roots(&mut gc_ctx);
    loop {
        let mut added = false;
        for root in collect_host_table_roots(&mut gc_ctx, &live_roots) {
            if access.resolve_handle(root).is_ok() && live_roots.insert(root) {
                added = true;
            }
        }
        if !added {
            break;
        }
    }

    let graph = build_object_graph(&access, handle_count, &mut gc_ctx);
    let young = YoungController::new(256);
    let old = OldController::new();
    populate_controllers(&young, &old, &graph);

    let root_snapshot = RootSnapshot::new(1, live_roots.iter().copied().collect());
    let mut director = GcDirector::new();
    director.update_space(access.free_bytes(), access.free_bytes() / 8);
    let young_bytes = estimate_generation_bytes(&graph, HandleGeneration::Young);
    let old_bytes = estimate_generation_bytes(&graph, HandleGeneration::Old);
    let decision = director.evaluate(young_bytes, old_bytes);
    let start_old = matches!(decision, DirectorDecision::StartOld)
        || matches!(decision, DirectorDecision::StartYoung)
        || old_bytes > 0;

    let mark_cpu_start = cpu_time::thread_cpu_ns();
    let pause_start = young.pause_mark_start(&root_snapshot);
    old.coordinate_from_young_mark_start(&young, &root_snapshot, start_old);

    while young.concurrent_mark_step(64) {}
    while old.concurrent_mark_step(64) {}

    let pause_end = young.pause_mark_end();
    let _ = old.pause_mark_end();
    let mark_cpu_ns = cpu_time::thread_cpu_ns().saturating_sub(mark_cpu_start);
    let relocate_cpu_start = cpu_time::thread_cpu_ns();
    let _sparse = young.select_relocation_set();
    apply_promotions(&access, &young, &old, graph.keys().copied());
    let pause_relocate = young.pause_relocate_start();
    young.finish_epoch_reclaim();
    let relocation_cpu_ns = cpu_time::thread_cpu_ns().saturating_sub(relocate_cpu_start);

    let mut live = live_roots;
    for handle in graph.keys().copied() {
        let id = HandleId::new(handle);
        if young.is_marked(id) || old.is_marked(id) {
            live.insert(handle);
        }
    }
    // 保险：从 roots 再走真实 heap 边，防止 controller 漏标。
    mark_reachable_on_heap(&access, handle_count, &mut live, &mut gc_ctx);

    let dead = access
        .live_handles(handle_count)
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

    let young_report = young.report();
    let old_report = old.report();
    let mark_live_bytes = graph
        .iter()
        .filter(|(handle, _)| {
            let id = HandleId::new(**handle);
            young.is_marked(id) || old.is_marked(id)
        })
        .map(|(_, (_, _, size))| *size)
        .sum::<u64>();
    let pause_ns = pause_start.as_nanos() as u64
        + pause_end.as_nanos() as u64
        + pause_relocate.as_nanos() as u64;

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
        cycle_kind: CycleKind::ZgcCycle,
        satb_flushes: young_report.satb_drained,
        rset_precise_slots: young_report.remset_slots_scanned,
        barrier_events: young_report.satb_drained + young_report.remset_slots_scanned,
        regions_old: old_report.marked,
        relocated_objects: young_report.relocated + young_report.promoted,
        scan_oblets: match decision {
            DirectorDecision::Idle => 0,
            DirectorDecision::StartYoung => 1,
            DirectorDecision::StartOld => 2,
            DirectorDecision::Continue => 3,
        },
        mark_cpu_ns,
        relocation_cpu_ns,
        mark_live_bytes,
        ..GcStats::default()
    };
    stats.free_bytes_reusable = stats.total_free_bytes;
    stats.record_pause(std::time::Duration::from_nanos(pause_ns.max(1)));
    stats.gc_cpu_ns = cpu_time::thread_cpu_ns().saturating_sub(gc_cpu_start);
    stats
}

type ObjectGraph = BTreeMap<u32, (HandleGeneration, Vec<Option<HandleId>>, u64)>;

fn build_object_graph(
    access: &HeapAccessV2,
    handle_count: u32,
    gc_ctx: &mut GcContext<'_>,
) -> ObjectGraph {
    let mut graph = BTreeMap::new();
    for handle in access.live_handles(handle_count) {
        let generation = access
            .handle_generation(handle)
            .unwrap_or(HandleGeneration::Young);
        let mut child_handles = Vec::new();
        if let Ok(references) = access.object_references(handle) {
            for raw in references {
                let mut children = Vec::new();
                object_walker::visit_value_handles(
                    gc_ctx,
                    raw,
                    handle_count as usize,
                    &mut |child| children.push(child),
                );
                for child in children {
                    if access.resolve_handle(child).is_ok() {
                        child_handles.push(Some(HandleId::new(child)));
                    }
                }
            }
        }
        let bytes = access.object_size_public(handle).unwrap_or(16);
        graph.insert(handle, (generation, child_handles, bytes));
    }
    graph
}

fn populate_controllers(young: &YoungController, old: &OldController, graph: &ObjectGraph) {
    for (handle, (generation, refs, bytes)) in graph {
        let id = HandleId::new(*handle);
        let dense = *bytes >= 1024;
        let humongous = *bytes >= 64 * 1024;
        young.register_object(id, *generation, refs.clone(), dense, humongous);
        old.register_object(id, *generation, refs.clone(), *bytes);
    }
}

fn apply_promotions(
    access: &HeapAccessV2,
    young: &YoungController,
    old: &OldController,
    handles: impl IntoIterator<Item = u32>,
) {
    for handle in handles {
        let id = HandleId::new(handle);
        if young.generation(id) == Some(HandleGeneration::Old) {
            let _ = access.promote_to_old(handle);
            old.note_promoted(id);
        }
    }
}

fn estimate_generation_bytes(graph: &ObjectGraph, generation: HandleGeneration) -> u64 {
    graph
        .values()
        .filter(|(object_gen, _, _)| *object_gen == generation)
        .map(|(_, _, bytes)| *bytes)
        .sum()
}

fn mark_reachable_on_heap(
    access: &HeapAccessV2,
    handle_count: u32,
    live: &mut HashSet<u32>,
    gc_ctx: &mut GcContext<'_>,
) {
    let mut pending: Vec<u32> = live.iter().copied().collect();
    while let Some(handle) = pending.pop() {
        let Ok(references) = access.object_references(handle) else {
            continue;
        };
        for raw in references {
            let mut children = Vec::new();
            object_walker::visit_value_handles(gc_ctx, raw, handle_count as usize, &mut |child| {
                children.push(child);
            });
            for child in children {
                if access.resolve_handle(child).is_ok() && live.insert(child) {
                    pending.push(child);
                }
            }
        }
    }
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
        roots.push(handle);
    });
    roots
}
