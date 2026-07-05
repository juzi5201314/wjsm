//! G1 增量标记 owner。
//!
//! 本模块维护 SATB 标记位图、worklist、final remark 与 old/humongous cleanup。
//! mixed evacuation 由后续 `mixed.rs` 接管，这里只释放全死 region。

use std::collections::HashSet;
use std::time::Instant;

use wasmtime::Val;

use crate::runtime_gc::api::{GcContext, GcStats, Handle, RootProvider, StepBudget};
use crate::runtime_gc::context::object_size_from_memory;
use crate::runtime_gc::mark_bitmap::MarkBitmap;
use crate::runtime_gc::object_walker::{self, ObjectWalker};
use crate::runtime_gc::{roots, weak_refs};

use super::region::{REGION_SIZE, RegionKind, RegionSpace};
use super::rset::G1RSet;

const DEFAULT_IHOP_PERCENT: usize = 45;
const GC_PHASE_IDLE: i32 = 0;
const GC_PHASE_MARK: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkPhase {
    Idle,
    Mark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarkStep {
    Progress { remaining_estimate: usize },
    ReadyForRemark,
}

#[derive(Debug)]
pub(super) struct ConcurrentMark {
    mark_bits: MarkBitmap,
    worklist: Vec<Handle>,
    phase: MarkPhase,
    cycle_epoch: u64,
    started_at: Option<Instant>,
    ihop_percent: usize,
}

#[derive(Clone, Copy, Debug)]
struct MarkObject {
    handle: Handle,
    ptr: usize,
    size: usize,
    kind: RegionKind,
    implicit_black: bool,
}

#[derive(Debug, Default)]
struct CleanupPlan {
    region_live_bytes: Vec<usize>,
    dead_handles: Vec<Handle>,
    release_regions: Vec<usize>,
    freed_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeadHandleCleanupStage {
    ClearObjTableSlots,
    ReclaimOwnerSideTables,
    ProcessWeakRefs,
    CleanupStreamTables,
    PublishHandles,
}

const DEAD_HANDLE_CLEANUP_ORDER: [DeadHandleCleanupStage; 5] = [
    DeadHandleCleanupStage::ClearObjTableSlots,
    DeadHandleCleanupStage::ReclaimOwnerSideTables,
    DeadHandleCleanupStage::ProcessWeakRefs,
    DeadHandleCleanupStage::CleanupStreamTables,
    DeadHandleCleanupStage::PublishHandles,
];

impl ConcurrentMark {
    pub(super) fn new() -> Self {
        Self {
            mark_bits: MarkBitmap::new(),
            worklist: Vec::new(),
            phase: MarkPhase::Idle,
            cycle_epoch: 0,
            started_at: None,
            ihop_percent: DEFAULT_IHOP_PERCENT,
        }
    }

    pub(super) fn is_active(&self) -> bool {
        self.phase == MarkPhase::Mark
    }

    pub(super) fn active_epoch(&self) -> Option<u64> {
        self.is_active().then_some(self.cycle_epoch)
    }

    pub(super) fn should_start(&self, ctx: &mut GcContext<'_>, regions: &RegionSpace) -> bool {
        if self.is_active() {
            return false;
        }
        let old_bytes = old_occupancy_bytes(ctx, regions);
        ihop_triggered(old_bytes, ctx.heap_limit(), self.ihop_percent)
    }

    pub(super) fn start_cycle(&mut self, ctx: &mut GcContext<'_>, rset: &mut G1RSet) -> u64 {
        self.phase = MarkPhase::Mark;
        self.cycle_epoch = self.cycle_epoch.saturating_add(1).max(1);
        self.started_at = Some(Instant::now());
        self.mark_bits.reset(ctx.obj_table_count());
        self.worklist.clear();
        let _ = rset.drain_satb_handles();
        self.cycle_epoch
    }

    pub(super) fn initial_mark(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots_provider: &mut dyn RootProvider,
        regions: &RegionSpace,
        rset: &mut G1RSet,
    ) {
        self.absorb_satb(ctx, rset);
        self.mark_roots_fixed_point(ctx, roots_provider, regions);
        set_wasm_phase(ctx, GC_PHASE_MARK);
    }

    pub(super) fn drain_incremental(
        &mut self,
        ctx: &mut GcContext<'_>,
        rset: &mut G1RSet,
        budget: StepBudget,
    ) -> MarkStep {
        self.absorb_satb(ctx, rset);
        self.drain_budget(ctx, budget.work_bytes, Some(budget.deadline));
        if self.worklist.is_empty() {
            MarkStep::ReadyForRemark
        } else {
            MarkStep::Progress {
                remaining_estimate: self.worklist.len().saturating_mul(64),
            }
        }
    }

    pub(super) fn finish_after_barrier_flush(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots_provider: &mut dyn RootProvider,
        regions: &mut RegionSpace,
        rset: &mut G1RSet,
    ) -> GcStats {
        self.absorb_satb(ctx, rset);
        self.drain_all(ctx);
        self.mark_roots_fixed_point(ctx, roots_provider, regions);
        self.absorb_satb(ctx, rset);
        self.drain_all(ctx);
        let stats = self.cleanup(ctx, regions);
        self.phase = MarkPhase::Idle;
        self.started_at = None;
        self.worklist.clear();
        set_wasm_phase(ctx, GC_PHASE_IDLE);
        ctx.stats = stats.clone();
        stats
    }

    fn mark_roots_fixed_point(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots_provider: &mut dyn RootProvider,
        regions: &RegionSpace,
    ) {
        let count = ctx.obj_table_count();
        let mut roots = Vec::new();
        roots_provider.for_each_shadow_stack_root(ctx, &mut |h| roots.push(h));
        roots_provider.for_each_wasm_local_root(ctx, &mut |h| roots.push(h));
        roots::for_each_immortal_region_root(ctx, &mut |h| roots.push(h));
        self.push_handles(count, roots);
        self.drain_all(ctx);

        let implicit = self.implicit_black_handles(ctx, regions);
        loop {
            let before = self.mark_bits.popcount();
            let mut host_roots = Vec::new();
            let mut is_marked = |h: Handle| self.mark_bits.is_marked(h) || implicit.contains(&h);
            roots_provider
                .for_each_host_table_root(ctx, &mut is_marked, &mut |h| host_roots.push(h));
            self.push_handles(count, host_roots);
            self.drain_all(ctx);
            if self.mark_bits.popcount() == before {
                break;
            }
        }
    }

    fn push_handles(&mut self, obj_table_count: usize, handles: impl IntoIterator<Item = Handle>) {
        for h in handles {
            if (h as usize) < obj_table_count && self.mark_bits.mark_if_new(h) {
                self.worklist.push(h);
            }
        }
    }

    fn absorb_satb(&mut self, ctx: &mut GcContext<'_>, rset: &mut G1RSet) {
        let count = ctx.obj_table_count();
        self.push_handles(count, rset.drain_satb_handles());
    }

    fn drain_all(&mut self, ctx: &mut GcContext<'_>) {
        self.drain_budget(ctx, usize::MAX, None);
    }

    fn drain_budget(
        &mut self,
        ctx: &mut GcContext<'_>,
        work_bytes: usize,
        deadline: Option<Instant>,
    ) {
        let obj_table_ptr = ctx.obj_table_ptr();
        let obj_table_count = ctx.obj_table_count();
        let mut walker = ObjectWalker::new();
        let mut processed = 0usize;
        while let Some(h) = self.worklist.pop() {
            let size = ctx.with_memory(|data| {
                object_walker::resolve_handle(data, h, obj_table_ptr, obj_table_count)
                    .and_then(|ptr| object_size_from_memory(data, ptr))
                    .unwrap_or(1)
            });
            walker.visit_object_children(ctx, h, obj_table_ptr, obj_table_count, &mut |child| {
                if self.mark_bits.mark_if_new(child) {
                    self.worklist.push(child);
                }
            });
            processed = processed.saturating_add(size.max(1));
            if processed >= work_bytes || deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
        }
    }

    fn implicit_black_handles(
        &self,
        ctx: &mut GcContext<'_>,
        regions: &RegionSpace,
    ) -> HashSet<Handle> {
        snapshot_mark_objects(ctx, regions, self.cycle_epoch)
            .into_iter()
            .filter_map(|object| object.implicit_black.then_some(object.handle))
            .collect()
    }

    fn cleanup(&mut self, ctx: &mut GcContext<'_>, regions: &mut RegionSpace) -> GcStats {
        let started_at = self.started_at.unwrap_or_else(Instant::now);
        let objects = snapshot_mark_objects(ctx, regions, self.cycle_epoch);
        let plan = build_cleanup_plan(&objects, regions, &self.mark_bits, self.cycle_epoch);
        for (idx, &bytes) in plan.region_live_bytes.iter().enumerate() {
            regions.set_live_bytes(idx, bytes);
        }
        release_dead_handles(ctx, &plan.dead_handles);
        for idx in plan.release_regions.iter().copied() {
            regions.release(idx);
        }
        if !plan.dead_handles.is_empty() || !plan.release_regions.is_empty() {
            ctx.increment_gc_epoch();
        }
        GcStats {
            marked: self.mark_bits.popcount(),
            swept: plan.dead_handles.len(),
            freed_bytes: plan.freed_bytes,
            elapsed: started_at.elapsed(),
            heap_used_bytes: ctx.heap_used(),
            ..GcStats::default()
        }
    }
}

impl Default for ConcurrentMark {
    fn default() -> Self {
        Self::new()
    }
}

fn set_wasm_phase(ctx: &mut GcContext<'_>, phase: i32) {
    if let Some(global) = ctx.env.gc_phase {
        let _ = global.set(&mut ctx.store, Val::I32(phase));
    }
}

fn old_occupancy_bytes(ctx: &mut GcContext<'_>, regions: &RegionSpace) -> usize {
    snapshot_mark_objects(ctx, regions, 0)
        .into_iter()
        .filter(|object| is_collectible_kind(object.kind))
        .map(|object| object.size)
        .sum()
}

fn ihop_triggered(old_bytes: usize, heap_limit: usize, percent: usize) -> bool {
    heap_limit != 0 && old_bytes.saturating_mul(100) >= heap_limit.saturating_mul(percent)
}

fn snapshot_mark_objects(
    ctx: &mut GcContext<'_>,
    regions: &RegionSpace,
    cycle_epoch: u64,
) -> Vec<MarkObject> {
    let obj_table_ptr = ctx.obj_table_ptr();
    let obj_table_count = ctx.obj_table_count();
    ctx.with_memory(|data| {
        let mut out = Vec::new();
        for h in 0..obj_table_count as Handle {
            let Some(ptr) = object_walker::resolve_handle(data, h, obj_table_ptr, obj_table_count)
            else {
                continue;
            };
            let Some(region_idx) = regions.region_index(ptr) else {
                continue;
            };
            let Some(region) = regions.region(region_idx) else {
                continue;
            };
            let Some(size) = object_size_from_memory(data, ptr) else {
                debug_assert!(false, "G1 mark: live obj_table entry has unreadable header");
                continue;
            };
            out.push(MarkObject {
                handle: h,
                ptr,
                size,
                kind: region.kind,
                implicit_black: region.implicit_black_epoch == cycle_epoch && cycle_epoch != 0,
            });
        }
        out
    })
}

fn build_cleanup_plan(
    objects: &[MarkObject],
    regions: &RegionSpace,
    mark_bits: &MarkBitmap,
    cycle_epoch: u64,
) -> CleanupPlan {
    let mut plan = CleanupPlan {
        region_live_bytes: vec![0; regions.region_count()],
        ..CleanupPlan::default()
    };
    for idx in 0..regions.region_count() {
        if regions.implicit_black_epoch(idx) == Some(cycle_epoch) && cycle_epoch != 0 {
            plan.region_live_bytes[idx] = REGION_SIZE;
        }
    }
    for object in objects {
        let dead = is_collectible_kind(object.kind)
            && !object.implicit_black
            && !mark_bits.is_marked(object.handle);
        if dead {
            plan.dead_handles.push(object.handle);
            plan.freed_bytes = plan.freed_bytes.saturating_add(object.size);
        } else if !object.implicit_black {
            add_live_bytes(
                &mut plan.region_live_bytes,
                regions,
                object.ptr,
                object.size,
            );
        }
    }
    for idx in 0..regions.region_count() {
        let Some(kind) = regions.kind(idx) else {
            continue;
        };
        if is_collectible_kind(kind)
            && plan.region_live_bytes.get(idx).copied().unwrap_or_default() == 0
            && regions.implicit_black_epoch(idx) != Some(cycle_epoch)
        {
            plan.release_regions.push(idx);
        }
    }
    plan
}

fn add_live_bytes(out: &mut [usize], regions: &RegionSpace, ptr: usize, size: usize) {
    let mut cursor = ptr;
    let mut remaining = size;
    while remaining != 0 {
        let Some(idx) = regions.region_index(cursor) else {
            break;
        };
        let Some(end) = regions.region_end(idx) else {
            break;
        };
        let chunk = remaining.min(end.saturating_sub(cursor));
        if let Some(slot) = out.get_mut(idx) {
            *slot = slot.saturating_add(chunk);
        }
        if chunk == 0 {
            break;
        }
        cursor = cursor.saturating_add(chunk);
        remaining -= chunk;
    }
}

fn is_collectible_kind(kind: RegionKind) -> bool {
    matches!(
        kind,
        RegionKind::Old | RegionKind::HumongousStart | RegionKind::HumongousCont
    )
}

fn release_dead_handles(ctx: &mut GcContext<'_>, dead_handles: &[Handle]) {
    if dead_handles.is_empty() {
        return;
    }
    let dead_set = dead_handles.iter().copied().collect::<HashSet<_>>();
    for stage in DEAD_HANDLE_CLEANUP_ORDER {
        match stage {
            DeadHandleCleanupStage::ClearObjTableSlots => {
                for &h in dead_handles {
                    ctx.write_obj_table_slot(h, 0);
                }
            }
            DeadHandleCleanupStage::ReclaimOwnerSideTables => {
                ctx.with_state(|st| {
                    st.reclaim_unmarked_collection_entries(|h| !dead_set.contains(&h));
                });
            }
            DeadHandleCleanupStage::ProcessWeakRefs => {
                weak_refs::process_weak_refs_after_sweep(ctx, dead_handles);
            }
            DeadHandleCleanupStage::CleanupStreamTables => {
                weak_refs::cleanup_stream_tables_after_sweep(ctx, dead_handles);
            }
            DeadHandleCleanupStage::PublishHandles => {
                ctx.with_state(|st| {
                    if let Some(mut list) = st.handle_free_list_for_gc() {
                        list.extend_from_slice(dead_handles);
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regions_with_old() -> RegionSpace {
        let start = REGION_SIZE;
        let mut regions = RegionSpace::default();
        regions.attach(start, start, start, start + 4 * REGION_SIZE);
        let eden = regions.region_index(start).unwrap();
        regions.set_kind_age(eden, RegionKind::Old, 0);
        regions
    }

    fn object(
        handle: Handle,
        region_idx: usize,
        kind: RegionKind,
        implicit_black: bool,
    ) -> MarkObject {
        MarkObject {
            handle,
            ptr: REGION_SIZE + region_idx * REGION_SIZE,
            size: 128,
            kind,
            implicit_black,
        }
    }

    #[test]
    fn g1_concurrent_mark_ihop_triggers_at_45_percent_old_occupancy() {
        assert!(ihop_triggered(45, 100, DEFAULT_IHOP_PERCENT));
        assert!(!ihop_triggered(44, 100, DEFAULT_IHOP_PERCENT));
    }

    #[test]
    fn g1_concurrent_mark_satb_keeps_overwritten_old_reference_live() {
        let regions = regions_with_old();
        let old_idx = regions.region_index(REGION_SIZE).unwrap();
        let objects = [object(7, old_idx, RegionKind::Old, false)];
        let mut bits = MarkBitmap::new();
        bits.reset(8);
        bits.mark(7);

        let plan = build_cleanup_plan(&objects, &regions, &bits, 1);

        assert!(plan.dead_handles.is_empty());
        assert_eq!(plan.region_live_bytes[old_idx], 128);
    }

    #[test]
    fn g1_concurrent_mark_initial_side_table_snapshot_keeps_removed_old_root() {
        let regions = regions_with_old();
        let old_idx = regions.region_index(REGION_SIZE).unwrap();
        let objects = [object(3, old_idx, RegionKind::Old, false)];
        let mut bits = MarkBitmap::new();
        bits.reset(4);
        bits.mark(3);

        let plan = build_cleanup_plan(&objects, &regions, &bits, 1);

        assert!(!plan.dead_handles.contains(&3));
    }

    #[test]
    fn g1_concurrent_mark_implicit_black_region_is_not_reclaimed_this_cycle() {
        let mut regions = regions_with_old();
        let old_idx = regions.region_index(REGION_SIZE).unwrap();
        regions.mark_implicit_black(old_idx, 9);
        let objects = [object(11, old_idx, RegionKind::Old, true)];
        let mut bits = MarkBitmap::new();
        bits.reset(12);

        let plan = build_cleanup_plan(&objects, &regions, &bits, 9);

        assert!(plan.dead_handles.is_empty());
        assert!(!plan.release_regions.contains(&old_idx));
        assert_eq!(plan.region_live_bytes[old_idx], REGION_SIZE);
    }

    #[test]
    fn g1_concurrent_mark_cleanup_publishes_handles_after_side_table_cleanup() {
        let publish = DEAD_HANDLE_CLEANUP_ORDER
            .iter()
            .position(|stage| *stage == DeadHandleCleanupStage::PublishHandles)
            .unwrap();
        let owner_cleanup = DEAD_HANDLE_CLEANUP_ORDER
            .iter()
            .position(|stage| *stage == DeadHandleCleanupStage::ReclaimOwnerSideTables)
            .unwrap();
        let weak_cleanup = DEAD_HANDLE_CLEANUP_ORDER
            .iter()
            .position(|stage| *stage == DeadHandleCleanupStage::ProcessWeakRefs)
            .unwrap();

        assert!(owner_cleanup < publish);
        assert!(weak_cleanup < publish);
        assert_eq!(publish, DEAD_HANDLE_CLEANUP_ORDER.len() - 1);
    }
}
