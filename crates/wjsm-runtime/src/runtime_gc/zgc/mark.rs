#![allow(dead_code)]
//! ZGC 增量标记 owner。
//!
//! 本模块维护本周期 mark bitmap、worklist、SATB 旧引用缓冲与 page live-bytes
//! 统计。relocation 由后续 `relocate.rs` 接管；这里在 MarkEnd 只负责判定死亡
//! handle、执行共享 cleanup，并把 handle 发布给 free-list。

use std::time::Instant;

use wasmtime::Val;
use wjsm_ir::{constants, value};

use crate::runtime_gc::api::{
    CycleKind, GcContext, GcStats, Handle, RootProvider, StepBudget, Value,
};
use crate::runtime_gc::context::object_size_from_memory;
use crate::runtime_gc::mark_bitmap::MarkBitmap;
use crate::runtime_gc::object_walker::{self, ObjectWalker};
use crate::runtime_gc::{roots, weak_refs};

use super::color::{PTR_MASK, ZColor, ZEntry};
use super::page::{ZPAGE_SIZE, ZPageSpace};

const BARRIER_EVENT_SIZE: usize = constants::GC_BARRIER_EVENT_SIZE as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum MarkStep {
    Progress { remaining_estimate: usize },
    ReadyForMarkEnd,
}

#[derive(Debug, Default)]
pub(crate) struct ZMarkState {
    mark_bits: MarkBitmap,
    worklist: Vec<Handle>,
    satb_handles: Vec<Handle>,
    active: bool,
    started_at: Option<Instant>,
    barrier_events: usize,
    satb_flushes: usize,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MarkObject {
    handle: Handle,
    ptr: usize,
    size: usize,
    color: ZColor,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct CleanupPlan {
    live_by_page: Vec<usize>,
    dead_handles: Vec<Handle>,
    freed_bytes: usize,
    live_handles: usize,
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

impl ZMarkState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn start_cycle(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots_provider: &mut dyn RootProvider,
        pages: &mut ZPageSpace,
        good: ZColor,
    ) {
        self.active = true;
        self.started_at = Some(Instant::now());
        self.mark_bits.reset(ctx.obj_table_count());
        self.worklist.clear();
        self.satb_handles.clear();
        self.barrier_events = 0;
        self.satb_flushes = 0;
        pages.reset_live_bytes();
        self.reset_stale_barrier_buffer(ctx);
        self.mark_roots_fixed_point(ctx, roots_provider, good);
    }

    #[allow(dead_code)]
    pub(crate) fn drain_incremental(
        &mut self,
        ctx: &mut GcContext<'_>,
        good: ZColor,
        budget: StepBudget,
    ) -> MarkStep {
        self.flush_barrier_buffer(ctx);
        self.absorb_satb(ctx, good);
        self.drain_budget(ctx, good, budget.work_bytes, Some(budget.deadline));
        if self.worklist.is_empty() {
            MarkStep::ReadyForMarkEnd
        } else {
            MarkStep::Progress {
                remaining_estimate: self.worklist.len().saturating_mul(64),
            }
        }
    }

    pub(crate) fn finish_after_barrier_flush(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots_provider: &mut dyn RootProvider,
        pages: &mut ZPageSpace,
        good: ZColor,
    ) -> GcStats {
        self.flush_barrier_buffer(ctx);
        self.absorb_satb(ctx, good);
        self.drain_all(ctx, good);
        self.mark_roots_fixed_point(ctx, roots_provider, good);
        self.flush_barrier_buffer(ctx);
        self.absorb_satb(ctx, good);
        self.drain_all(ctx, good);
        let stats = self.cleanup(ctx, pages, good);
        self.active = false;
        self.started_at = None;
        self.worklist.clear();
        self.satb_handles.clear();
        ctx.stats = stats.clone();
        stats
    }

    pub(crate) fn mark_from_load_barrier(
        &mut self,
        ctx: &mut GcContext<'_>,
        h: Handle,
        good: ZColor,
    ) -> u32 {
        let Some((slot, entry)) = read_entry(ctx, h) else {
            return 0;
        };
        if entry.is_empty() {
            return 0;
        }
        let repaired = entry.repair_bad_non_relocating(good);
        if repaired.raw() != entry.raw() {
            write_entry(ctx, slot, repaired);
            ctx.increment_gc_epoch();
        }
        if self.mark_bits.mark_if_new(h) {
            self.worklist.push(h);
        }
        repaired.raw()
    }

    pub(crate) fn record_host_write(&mut self, ctx: &mut GcContext<'_>, old_val: Value) {
        if self.active {
            self.barrier_events = self.barrier_events.saturating_add(1);
            self.record_satb_value(ctx, old_val);
        }
    }

    pub(crate) fn flush_barrier_buffer(&mut self, ctx: &mut GcContext<'_>) {
        let Some((base, ptr)) = barrier_buffer_range(ctx) else {
            return;
        };
        if ptr <= base {
            return;
        }
        if self.active {
            let old_values = ctx.with_memory(|data| {
                data.get(base..ptr)
                    .map(|buf| decode_buffer_old_values(buf).collect::<Vec<_>>())
                    .unwrap_or_default()
            });
            if !old_values.is_empty() {
                self.barrier_events = self.barrier_events.saturating_add(old_values.len());
            }
            for old in old_values {
                self.record_satb_value(ctx, old);
            }
        }
        reset_barrier_buffer(ctx, base);
    }

    fn reset_stale_barrier_buffer(&mut self, ctx: &mut GcContext<'_>) {
        if let Some((base, _)) = barrier_buffer_range(ctx) {
            reset_barrier_buffer(ctx, base);
        }
    }

    fn mark_roots_fixed_point(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots_provider: &mut dyn RootProvider,
        good: ZColor,
    ) {
        let count = ctx.obj_table_count();
        let mut roots = Vec::new();
        roots_provider.for_each_shadow_stack_root(ctx, &mut |h| roots.push(h));
        roots_provider.for_each_wasm_local_root(ctx, &mut |h| roots.push(h));
        roots::for_each_immortal_region_root(ctx, &mut |h| roots.push(h));
        self.push_handles(ctx, count, good, roots);
        self.drain_all(ctx, good);

        loop {
            let before = self.mark_bits.popcount();
            let mut host_roots = Vec::new();
            let mut is_marked = |h: Handle| self.mark_bits.is_marked(h);
            roots_provider.for_each_host_table_root(ctx, &mut is_marked, &mut |h| {
                host_roots.push(h);
            });
            self.push_handles(ctx, count, good, host_roots);
            self.drain_all(ctx, good);
            if self.mark_bits.popcount() == before {
                break;
            }
        }
    }

    fn push_handles(
        &mut self,
        ctx: &mut GcContext<'_>,
        obj_table_count: usize,
        good: ZColor,
        handles: impl IntoIterator<Item = Handle>,
    ) {
        for h in handles {
            if (h as usize) < obj_table_count {
                self.push_handle(ctx, h, good);
            }
        }
    }

    fn push_handle(&mut self, ctx: &mut GcContext<'_>, h: Handle, good: ZColor) {
        if !repair_entry_to_good(ctx, h, good) {
            return;
        }
        if self.mark_bits.mark_if_new(h) {
            self.worklist.push(h);
        }
    }

    fn absorb_satb(&mut self, ctx: &mut GcContext<'_>, good: ZColor) {
        if self.satb_handles.is_empty() {
            return;
        }
        self.satb_flushes = self.satb_flushes.saturating_add(1);
        let count = ctx.obj_table_count();
        let handles = std::mem::take(&mut self.satb_handles);
        self.push_handles(ctx, count, good, handles);
    }

    fn record_satb_value(&mut self, ctx: &mut GcContext<'_>, val: Value) {
        let count = ctx.obj_table_count();
        let mut handles = Vec::new();
        object_walker::visit_value_handles(ctx, val, count, &mut |h| handles.push(h));
        self.satb_handles.extend(handles);
    }

    fn drain_all(&mut self, ctx: &mut GcContext<'_>, good: ZColor) {
        self.drain_budget(ctx, good, usize::MAX, None);
    }

    fn drain_budget(
        &mut self,
        ctx: &mut GcContext<'_>,
        good: ZColor,
        work_bytes: usize,
        deadline: Option<Instant>,
    ) {
        let obj_table_ptr = ctx.obj_table_ptr();
        let obj_table_count = ctx.obj_table_count();
        let mut walker = ObjectWalker::new();
        let mut processed = 0usize;
        let mut children = Vec::new();
        while let Some(h) = self.worklist.pop() {
            let size = ctx.with_memory(|data| {
                object_walker::resolve_handle(data, h, obj_table_ptr, obj_table_count)
                    .and_then(|ptr| object_size_from_memory(data, ptr))
                    .unwrap_or(1)
            });
            children.clear();
            walker.visit_object_children(ctx, h, obj_table_ptr, obj_table_count, &mut |child| {
                children.push(child);
            });
            for child in children.drain(..) {
                self.push_handle(ctx, child, good);
            }
            processed = processed.saturating_add(size.max(1));
            if processed >= work_bytes || deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
        }
    }

    fn cleanup(
        &mut self,
        ctx: &mut GcContext<'_>,
        pages: &mut ZPageSpace,
        good: ZColor,
    ) -> GcStats {
        let started_at = self.started_at.unwrap_or_else(Instant::now);
        let plan = build_cleanup_plan_from_heap(ctx, pages, &self.mark_bits, good);
        pages.reset_live_bytes();
        for (idx, &bytes) in plan.live_by_page.iter().enumerate() {
            pages.set_live_bytes(idx, bytes);
        }
        let reclaimed_pages = pages.reclaim_dead_pages();
        release_dead_handles(ctx, &plan.dead_handles);
        if !plan.dead_handles.is_empty() || !reclaimed_pages.is_empty() {
            ctx.increment_gc_epoch();
        }
        let metrics =
            crate::runtime_gc::heap_governance::compute_metrics(&pages.free_page_intervals());
        GcStats {
            marked: plan.live_handles,
            swept: plan.dead_handles.len(),
            freed_bytes: plan.freed_bytes,
            elapsed: started_at.elapsed(),
            free_block_count: metrics.free_block_count,
            total_free_bytes: metrics.total_free_bytes,
            largest_free_block: metrics.largest_free_block,
            external_fragmentation: metrics.external_fragmentation,
            heap_used_bytes: ctx.heap_used(),
            cycle_kind: CycleKind::ZgcCycle,
            committed_pages: ctx.committed_pages(),
            free_bytes_reusable: metrics.total_free_bytes,
            satb_flushes: self.satb_flushes,
            barrier_events: self.barrier_events,
            ..GcStats::default()
        }
        .with_elapsed_pause()
    }
}

pub(crate) fn obj_table_slot_addr(ctx: &mut GcContext<'_>, h: Handle) -> Option<usize> {
    if h as usize >= ctx.obj_table_count() {
        return None;
    }
    ctx.obj_table_ptr()
        .checked_add(h as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize)
}

pub(crate) fn read_entry(ctx: &mut GcContext<'_>, h: Handle) -> Option<(usize, ZEntry)> {
    let slot = obj_table_slot_addr(ctx, h)?;
    let raw = ctx.with_memory(|data| {
        let bytes: [u8; 4] = data.get(slot..slot + 4)?.try_into().ok()?;
        Some(u32::from_le_bytes(bytes))
    })?;
    Some((slot, zentry_from_raw(raw)))
}

pub(crate) fn write_entry(ctx: &mut GcContext<'_>, slot: usize, entry: ZEntry) {
    let raw = entry.raw().to_le_bytes();
    ctx.with_memory_mut(|data| {
        if let Some(dst) = data.get_mut(slot..slot + 4) {
            dst.copy_from_slice(&raw);
        }
    });
}

fn zentry_from_raw(raw: u32) -> ZEntry {
    if raw == 0 {
        ZEntry::empty()
    } else {
        ZEntry::new(
            raw & PTR_MASK,
            ZColor::from_bits(raw).unwrap_or(ZColor::Empty),
        )
    }
}

fn repair_entry_to_good(ctx: &mut GcContext<'_>, h: Handle, good: ZColor) -> bool {
    let Some((slot, entry)) = read_entry(ctx, h) else {
        return false;
    };
    if entry.is_empty() {
        return false;
    }
    let repaired = entry.repair_bad_non_relocating(good);
    if repaired.raw() != entry.raw() {
        write_entry(ctx, slot, repaired);
        ctx.increment_gc_epoch();
    }
    true
}

fn barrier_buffer_range(ctx: &mut GcContext<'_>) -> Option<(usize, usize)> {
    let (_, _, base) = ctx.with_state(|state| state.heap_layout_boundaries());
    if base == 0 {
        return None;
    }
    let ptr = ctx
        .env
        .barrier_buf_ptr?
        .get(&mut ctx.store)
        .i32()
        .unwrap_or(base as i32)
        .max(0) as usize;
    Some((base, ptr))
}

fn reset_barrier_buffer(ctx: &mut GcContext<'_>, base: usize) {
    if let Some(global) = ctx.env.barrier_buf_ptr {
        let _ = global.set(&mut ctx.store, Val::I32(base as i32));
    }
}

fn decode_buffer_old_values(input: &[u8]) -> impl Iterator<Item = Value> + '_ {
    input.chunks_exact(BARRIER_EVENT_SIZE).filter_map(|chunk| {
        let flags = u32::from_le_bytes(chunk[0..4].try_into().ok()?);
        if flags & 1 == 0 {
            return None;
        }
        let old = i64::from_le_bytes(chunk[8..16].try_into().ok()?);
        value::tag_needs_root(old).then_some(old)
    })
}

#[allow(dead_code)]
#[cfg(test)]
fn snapshot_mark_objects(ctx: &mut GcContext<'_>, pages: &ZPageSpace) -> Vec<MarkObject> {
    let obj_table_ptr = ctx.obj_table_ptr();
    let obj_table_count = ctx.obj_table_count();
    ctx.with_memory(|data| {
        let mut out = Vec::new();
        for h in 0..obj_table_count as Handle {
            let slot = obj_table_ptr + h as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize;
            let Some(bytes) = data.get(slot..slot + 4) else {
                break;
            };
            let entry = zentry_from_raw(u32::from_le_bytes(bytes.try_into().unwrap()));
            if entry.is_empty() {
                continue;
            }
            let ptr = entry.ptr() as usize;
            if pages.page_index(ptr).is_none() {
                continue;
            }
            let Some(size) = object_size_from_memory(data, ptr) else {
                debug_assert!(
                    false,
                    "ZGC mark: live obj_table entry has unreadable header"
                );
                continue;
            };
            out.push(MarkObject {
                handle: h,
                ptr,
                size,
                color: entry.color(),
            });
        }
        out
    })
}

fn build_cleanup_plan_from_heap(
    ctx: &mut GcContext<'_>,
    pages: &ZPageSpace,
    mark_bits: &MarkBitmap,
    good: ZColor,
) -> CleanupPlan {
    let obj_table_ptr = ctx.obj_table_ptr();
    let obj_table_count = ctx.obj_table_count();
    ctx.with_memory(|data| {
        let mut plan = CleanupPlan {
            live_by_page: vec![0; pages.page_count()],
            ..CleanupPlan::default()
        };
        for h in 0..obj_table_count as Handle {
            let slot = obj_table_ptr + h as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize;
            let Some(bytes) = data.get(slot..slot + 4) else {
                break;
            };
            let entry = zentry_from_raw(u32::from_le_bytes(bytes.try_into().unwrap()));
            if entry.is_empty() {
                continue;
            }
            let ptr = entry.ptr() as usize;
            if pages.page_index(ptr).is_none() {
                continue;
            }
            let Some(size) = object_size_from_memory(data, ptr) else {
                debug_assert!(
                    false,
                    "ZGC mark: live obj_table entry has unreadable header"
                );
                continue;
            };
            let live = mark_bits.is_marked(h) || entry.color() == good;
            if live {
                plan.live_handles = plan.live_handles.saturating_add(1);
                add_live_bytes(&mut plan.live_by_page, pages, ptr, size);
            } else {
                plan.dead_handles.push(h);
                plan.freed_bytes = plan.freed_bytes.saturating_add(size);
            }
        }
        plan
    })
}

#[cfg(test)]
fn build_cleanup_plan(
    objects: &[MarkObject],
    pages: &ZPageSpace,
    mark_bits: &MarkBitmap,
    good: ZColor,
) -> CleanupPlan {
    let mut plan = CleanupPlan {
        live_by_page: vec![0; pages.page_count()],
        ..CleanupPlan::default()
    };
    for object in objects {
        let live = mark_bits.is_marked(object.handle) || object.color == good;
        if live {
            plan.live_handles = plan.live_handles.saturating_add(1);
            add_live_bytes(&mut plan.live_by_page, pages, object.ptr, object.size);
        } else {
            plan.dead_handles.push(object.handle);
            plan.freed_bytes = plan.freed_bytes.saturating_add(object.size);
        }
    }
    plan
}

fn add_live_bytes(out: &mut [usize], pages: &ZPageSpace, ptr: usize, size: usize) {
    let mut cursor = ptr;
    let mut remaining = size;
    while remaining != 0 {
        let Some(idx) = pages.page_index(cursor) else {
            break;
        };
        let Some(start) = pages.page_start(idx) else {
            break;
        };
        let end = start.saturating_add(ZPAGE_SIZE);
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

fn release_dead_handles(ctx: &mut GcContext<'_>, dead_handles: &[Handle]) {
    if dead_handles.is_empty() {
        return;
    }
    debug_assert!(dead_handles.windows(2).all(|pair| pair[0] < pair[1]));
    for stage in DEAD_HANDLE_CLEANUP_ORDER {
        match stage {
            DeadHandleCleanupStage::ClearObjTableSlots => {
                for &h in dead_handles {
                    ctx.write_obj_table_slot(h, 0);
                }
            }
            DeadHandleCleanupStage::ReclaimOwnerSideTables => {
                ctx.with_state(|st| {
                    st.reclaim_unmarked_collection_entries(|h| {
                        dead_handles.binary_search(&h).is_err()
                    });
                    crate::realm::reclaim_dead_realms(st, |h| {
                        dead_handles.binary_search(&h).is_err()
                    });
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
    use wjsm_ir::value;

    fn object(handle: Handle, page_idx: usize, color: ZColor) -> MarkObject {
        MarkObject {
            handle,
            ptr: page_idx * ZPAGE_SIZE,
            size: 128,
            color,
        }
    }

    #[test]
    fn zgc_mark_bad_color_hit_keeps_object_live() {
        let mut pages = ZPageSpace::default();
        pages.attach(0, ZPAGE_SIZE);
        let objects = [object(3, 0, ZColor::Remapped)];
        let mut bits = MarkBitmap::new();
        bits.reset(4);
        bits.mark(3);

        let plan = build_cleanup_plan(&objects, &pages, &bits, ZColor::Marked1);

        assert!(plan.dead_handles.is_empty());
        assert_eq!(plan.live_by_page[0], 128);
    }

    #[test]
    fn zgc_mark_satb_keeps_overwritten_old_reference_live() {
        let mut pages = ZPageSpace::default();
        pages.attach(0, ZPAGE_SIZE);
        let objects = [object(7, 0, ZColor::Marked0)];
        let mut bits = MarkBitmap::new();
        bits.reset(8);
        bits.mark(7);

        let plan = build_cleanup_plan(&objects, &pages, &bits, ZColor::Marked1);

        assert!(plan.dead_handles.is_empty());
        assert_eq!(plan.live_by_page[0], 128);
    }

    #[test]
    fn zgc_mark_dead_handle_set_covers_all_unmarked_bad_color_objects() {
        let mut pages = ZPageSpace::default();
        pages.attach(0, 3 * ZPAGE_SIZE);
        let objects = [
            object(1, 0, ZColor::Marked0),
            object(2, 1, ZColor::Marked1),
            object(3, 2, ZColor::Remapped),
        ];
        let mut bits = MarkBitmap::new();
        bits.reset(4);
        bits.mark(3);

        let plan = build_cleanup_plan(&objects, &pages, &bits, ZColor::Marked1);

        assert_eq!(plan.dead_handles, vec![1]);
        assert_eq!(plan.live_by_page, vec![0, 128, 128]);
        assert_eq!(plan.freed_bytes, 128);
    }

    #[test]
    fn zgc_mark_cleanup_publishes_handles_after_weak_and_owner_cleanup() {
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
        let finalization_enqueue = weak_cleanup;

        assert!(owner_cleanup < publish);
        assert!(weak_cleanup < publish);
        assert!(finalization_enqueue < publish);
        assert_eq!(publish, DEAD_HANDLE_CLEANUP_ORDER.len() - 1);
    }

    #[test]
    fn zgc_mark_decodes_only_satb_barrier_events() {
        let mut buf = vec![0u8; BARRIER_EVENT_SIZE * 3];
        buf[0..4].copy_from_slice(&1u32.to_le_bytes());
        buf[8..16].copy_from_slice(&value::encode_object_handle(9).to_le_bytes());
        buf[BARRIER_EVENT_SIZE..BARRIER_EVENT_SIZE + 4].copy_from_slice(&2u32.to_le_bytes());
        buf[BARRIER_EVENT_SIZE + 8..BARRIER_EVENT_SIZE + 16]
            .copy_from_slice(&value::encode_object_handle(10).to_le_bytes());
        let number_event = BARRIER_EVENT_SIZE * 2;
        buf[number_event..number_event + 4].copy_from_slice(&1u32.to_le_bytes());
        buf[number_event + 8..number_event + 16]
            .copy_from_slice(&(42.0f64.to_bits() as i64).to_le_bytes());

        let old_values = decode_buffer_old_values(&buf).collect::<Vec<_>>();

        assert_eq!(old_values, vec![value::encode_object_handle(9)]);
    }
}
