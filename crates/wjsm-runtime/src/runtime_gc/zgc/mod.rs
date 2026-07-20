pub mod color;
mod mark;
pub mod page;
mod relocate;
#[cfg(feature = "managed-heap-v2")]
pub mod barrier;
#[cfg(feature = "managed-heap-v2")]
pub mod concurrent_relocate;
#[cfg(feature = "managed-heap-v2")]
pub mod host_roots;
#[cfg(feature = "managed-heap-v2")]
pub mod old;
#[cfg(feature = "managed-heap-v2")]
pub mod remset;
#[cfg(feature = "managed-heap-v2")]
pub mod young;
#[cfg(feature = "managed-heap-v2")]
mod v2;
#[cfg(feature = "managed-heap-v2")]
pub use barrier::{
    BarrierEpoch, BarrierRecord, BarrierRing, BulkCopyMode, HeaderField, HeaderFieldKind,
    HeaderLayout, LoadBarrierOutcome, classify_entry, color_stored_value, load_barrier,
    prototype_field_kind, select_bulk_copy_mode, store_barrier, store_barrier_with_target_generation,
};
#[cfg(feature = "managed-heap-v2")]
pub use concurrent_relocate::{
    ConcurrentRelocator, PageRelocationState, RelocationDescriptor, RelocationReport,
};
#[cfg(feature = "managed-heap-v2")]
pub use host_roots::{ConcurrentHostRoots, HostRootsReport, WeakState};
#[cfg(feature = "managed-heap-v2")]
pub use old::{OldController, OldPhase, OldReport};
#[cfg(feature = "managed-heap-v2")]
pub use remset::{PreciseRemset, publish_promotion};
#[cfg(feature = "managed-heap-v2")]
pub use young::{YoungController, YoungPhase, YoungReport};
#[cfg(feature = "managed-heap-v2")]
pub use v2::{ZgcV2, ZgcV2Error, ZgcV2Phase, ZgcV2Report, ZgcV2StepOutcome};

use super::api::{
    AllocRequest, GcAlgorithm, GcContext, GcStats, Handle, RootProvider, StepBudget, StepOutcome,
    Value,
};
use color::{PTR_MASK, ZColor, ZColorState, ZEntry, ZPhase};
use mark::{MarkStep, ZMarkState};
use page::{ZPageSpace, recolor_live_obj_table_entries};
use relocate::{RelocateStep, ZRelocateState};
use std::time::{Duration, Instant};
use wasmtime::Val;
use wjsm_ir::constants;

pub(crate) struct ZgcCollector {
    colors: ZColorState,
    pages: ZPageSpace,
    mark: ZMarkState,
    relocate: ZRelocateState,
    stats: GcStats,
    load_barrier_mark_hits: usize,
    load_barrier_relocate_hits: usize,
}

impl ZgcCollector {
    pub(crate) fn new() -> Self {
        Self {
            colors: ZColorState::default(),
            pages: ZPageSpace::default(),
            mark: ZMarkState::new(),
            relocate: ZRelocateState::new(),
            stats: GcStats::default(),
            load_barrier_mark_hits: 0,
            load_barrier_relocate_hits: 0,
        }
    }

    fn committed_end(ctx: &mut GcContext<'_>) -> usize {
        ctx.heap_limit().min(ctx.env.memory.data_size(&ctx.store))
    }

    fn attach_pages_and_recolor(&mut self, ctx: &mut GcContext<'_>) {
        let (_, dynamic_start, _) = ctx.with_state(|state| state.heap_layout_boundaries());
        self.pages.attach(dynamic_start, Self::committed_end(ctx));
        let good = self.colors.good();
        let obj_table_ptr = ctx.obj_table_ptr();
        let obj_table_count = ctx.obj_table_count();
        let recolored = ctx.with_memory_mut(|data| {
            recolor_live_obj_table_entries(data, obj_table_ptr, obj_table_count, good)
        });
        if recolored != 0 {
            ctx.increment_gc_epoch();
        }
        self.sync_wasm_color_phase(ctx);
    }

    fn refresh_pages(&mut self, ctx: &mut GcContext<'_>) {
        let (_, dynamic_start, _) = ctx.with_state(|state| state.heap_layout_boundaries());
        self.pages.attach(dynamic_start, Self::committed_end(ctx));
    }

    fn start_mark_cycle(&mut self, ctx: &mut GcContext<'_>, roots: &mut dyn RootProvider) {
        self.refresh_pages(ctx);
        self.load_barrier_mark_hits = 0;
        self.load_barrier_relocate_hits = 0;
        let good = self.colors.start_mark();
        self.sync_wasm_color_phase(ctx);
        self.mark.start_cycle(ctx, roots, &mut self.pages, good);
    }

    fn finish_active_mark(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots: &mut dyn RootProvider,
        copy_budget: usize,
    ) -> Option<GcStats> {
        if !self.mark.is_active() {
            return None;
        }
        let good = self.colors.good();
        let mut stats = self
            .mark
            .finish_after_barrier_flush(ctx, roots, &mut self.pages, good);
        stats.load_barrier_mark_hits = self.load_barrier_mark_hits;
        self.stats = stats.clone();
        let excluded = self.pages.page_index(ctx.heap_ptr());
        if self
            .relocate
            .start_cycle_excluding(&mut self.pages, copy_budget, excluded)
        {
            self.colors.start_relocate();
        } else {
            self.colors.finish_cycle();
        }
        self.sync_wasm_color_phase(ctx);
        Some(stats)
    }

    fn merge_stats(primary: &mut GcStats, extra: &GcStats) {
        primary.merge_from(extra);
    }

    fn finish_relocate_step(
        &mut self,
        ctx: &mut GcContext<'_>,
        mut stats: Box<GcStats>,
    ) -> StepOutcome {
        self.colors.finish_cycle();
        self.sync_wasm_color_phase(ctx);
        stats.load_barrier_relocate_hits = self.load_barrier_relocate_hits;
        Self::merge_stats(&mut self.stats, &stats);
        ctx.stats = self.stats.clone();
        StepOutcome::CycleComplete
    }

    fn unbounded_budget() -> StepBudget {
        StepBudget {
            work_bytes: usize::MAX,
            deadline: Instant::now() + Duration::from_secs(24 * 60 * 60),
        }
    }

    fn alloc_from_bump(&mut self, ctx: &mut GcContext<'_>, size: usize) -> Option<usize> {
        let (_, dynamic_start, _) = ctx.with_state(|state| state.heap_layout_boundaries());
        let ptr = ctx.heap_ptr().max(dynamic_start);
        let end = ptr.checked_add(size)?;
        let align = constants::HEAP_ALLOCATION_ALIGNMENT as usize;
        let aligned_end = end.checked_add(align - 1).map(|v| v & !(align - 1))?;
        if aligned_end > ctx.heap_limit() {
            return None;
        }
        if aligned_end > ctx.env.memory.data_size(&ctx.store) {
            let grow_bytes = aligned_end.checked_sub(ptr)?;
            if !matches!(ctx.grow_to_fit_heap_allocation(grow_bytes), Ok(true)) {
                return None;
            }
        }
        ctx.set_heap_ptr(aligned_end);
        let alloc_end = ctx.env.memory.data_size(&ctx.store).min(ctx.heap_limit());
        ctx.alloc_window_set(aligned_end, alloc_end);
        self.refresh_pages(ctx);
        Some(ptr)
    }

    fn sync_alloc_window(&mut self, ctx: &mut GcContext<'_>) {
        let (_, dynamic_start, _) = ctx.with_state(|state| state.heap_layout_boundaries());
        let ptr = ctx.heap_ptr().max(dynamic_start);
        if ptr != ctx.heap_ptr() {
            ctx.set_heap_ptr(ptr);
        }
        let alloc_end = ctx.env.memory.data_size(&ctx.store).min(ctx.heap_limit());
        ctx.alloc_window_set(ptr, alloc_end);
    }

    fn sync_wasm_color_phase(&self, ctx: &mut GcContext<'_>) {
        if let Some(global) = ctx.env.good_color {
            let _ = global.set(&mut ctx.store, Val::I32(self.colors.good().bits() as i32));
        }
        if let Some(global) = ctx.env.gc_phase {
            let phase = match self.colors.phase() {
                ZPhase::Idle => 0,
                ZPhase::Mark => 1,
                ZPhase::Relocate => 2,
            };
            let _ = global.set(&mut ctx.store, Val::I32(phase));
        }
    }

    fn obj_table_slot_addr(ctx: &mut GcContext<'_>, h: Handle) -> Option<usize> {
        if h as usize >= ctx.obj_table_count() {
            return None;
        }
        ctx.obj_table_ptr()
            .checked_add(h as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize)
    }

    fn read_entry(ctx: &mut GcContext<'_>, h: Handle) -> Option<(usize, ZEntry)> {
        let slot = Self::obj_table_slot_addr(ctx, h)?;
        let raw = ctx.with_memory(|data| {
            let bytes: [u8; 4] = data.get(slot..slot + 4)?.try_into().ok()?;
            Some(u32::from_le_bytes(bytes))
        })?;
        if raw == 0 {
            return Some((slot, ZEntry::empty()));
        }
        let color = ZColor::from_bits(raw).unwrap_or(ZColor::Empty);
        Some((slot, ZEntry::new(raw & PTR_MASK, color)))
    }

    fn write_entry(ctx: &mut GcContext<'_>, slot: usize, entry: ZEntry) {
        let raw = entry.raw().to_le_bytes();
        ctx.with_memory_mut(|data| {
            if let Some(dst) = data.get_mut(slot..slot + 4) {
                dst.copy_from_slice(&raw);
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn start_mark_for_tests(&mut self) -> ZColor {
        self.colors.start_mark()
    }

    #[cfg(test)]
    pub(crate) fn start_relocate_for_tests(&mut self) -> ZColor {
        self.colors.start_relocate()
    }
}

impl GcAlgorithm for ZgcCollector {
    fn name(&self) -> &'static str {
        "zgc"
    }

    fn attach_heap(&mut self, ctx: &mut GcContext<'_>, _dynamic_start: usize) {
        self.attach_pages_and_recolor(ctx);
        self.sync_alloc_window(ctx);
    }

    fn alloc_slow(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots: &mut dyn RootProvider,
        req: AllocRequest,
    ) -> Option<usize> {
        self.refresh_pages(ctx);
        if self.relocate.is_active() {
            let budget = StepBudget {
                work_bytes: req.size.max(64 * 1024),
                deadline: Instant::now() + Duration::from_millis(2),
            };
            if let RelocateStep::Complete { stats } =
                self.relocate
                    .drain_incremental(ctx, &mut self.pages, budget)
            {
                let _ = self.finish_relocate_step(ctx, stats);
            }
        }
        if let Some(ptr) = self.alloc_from_bump(ctx, req.size) {
            return Some(ptr);
        }
        let _ = self.collect_full(ctx, roots);
        self.alloc_from_bump(ctx, req.size)
    }

    fn safepoint_step(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots: &mut dyn RootProvider,
        budget: StepBudget,
    ) -> StepOutcome {
        self.refresh_pages(ctx);
        if self.mark.is_active() {
            return match self.mark.drain_incremental(ctx, self.colors.good(), budget) {
                MarkStep::Progress { remaining_estimate } => {
                    StepOutcome::Progress { remaining_estimate }
                }
                MarkStep::ReadyForMarkEnd => {
                    let _ = self.finish_active_mark(ctx, roots, budget.work_bytes);
                    if self.relocate.is_active() {
                        StepOutcome::Progress {
                            remaining_estimate: budget.work_bytes.max(1),
                        }
                    } else {
                        StepOutcome::CycleComplete
                    }
                }
            };
        }
        if self.relocate.is_active() {
            return match self
                .relocate
                .drain_incremental(ctx, &mut self.pages, budget)
            {
                RelocateStep::Idle => StepOutcome::Idle,
                RelocateStep::Progress { remaining_estimate } => {
                    StepOutcome::Progress { remaining_estimate }
                }
                RelocateStep::Complete { stats } => self.finish_relocate_step(ctx, stats),
            };
        }
        self.start_mark_cycle(ctx, roots);
        StepOutcome::Progress {
            remaining_estimate: budget.work_bytes.max(1),
        }
    }

    fn collect_full(&mut self, ctx: &mut GcContext<'_>, roots: &mut dyn RootProvider) -> GcStats {
        self.refresh_pages(ctx);
        if !self.relocate.is_active() {
            if !self.mark.is_active() {
                self.start_mark_cycle(ctx, roots);
            }
            let _ = self.finish_active_mark(ctx, roots, usize::MAX);
        }
        while self.relocate.is_active() {
            match self
                .relocate
                .drain_incremental(ctx, &mut self.pages, Self::unbounded_budget())
            {
                RelocateStep::Complete { stats } => {
                    let _ = self.finish_relocate_step(ctx, stats);
                }
                RelocateStep::Idle => break,
                RelocateStep::Progress { .. } => break,
            }
        }
        ctx.stats = self.stats.clone();
        self.stats.clone()
    }

    fn load_barrier_slow(&mut self, ctx: &mut GcContext<'_>, h: Handle) -> u32 {
        match self.colors.phase() {
            ZPhase::Mark => {
                let entry = self.mark.mark_from_load_barrier(ctx, h, self.colors.good());
                if entry != 0 {
                    self.load_barrier_mark_hits = self.load_barrier_mark_hits.saturating_add(1);
                }
                return entry;
            }
            ZPhase::Relocate => {
                let entry = self
                    .relocate
                    .relocate_or_remap_handle(ctx, &mut self.pages, h);
                if entry != 0 {
                    self.load_barrier_relocate_hits =
                        self.load_barrier_relocate_hits.saturating_add(1);
                }
                return entry;
            }
            ZPhase::Idle => {}
        }
        let Some((slot, entry)) = Self::read_entry(ctx, h) else {
            return 0;
        };
        if entry.is_empty() {
            return 0;
        }
        let repaired = if self.colors.good() == ZColor::Remapped {
            entry.repair_relocate_non_rs()
        } else {
            entry.repair_bad_non_relocating(self.colors.good())
        };
        if repaired.raw() != entry.raw() {
            Self::write_entry(ctx, slot, repaired);
            ctx.increment_gc_epoch();
        }
        repaired.raw()
    }

    fn barrier_flush(&mut self, ctx: &mut GcContext<'_>) {
        self.mark.flush_barrier_buffer(ctx);
    }

    fn on_host_write(
        &mut self,
        ctx: &mut GcContext<'_>,
        _target: Handle,
        _slot_addr: usize,
        old_val: Value,
        _new_val: Value,
    ) {
        self.mark.record_host_write(ctx, old_val);
    }

    fn on_host_resolve(&mut self, ctx: &mut GcContext<'_>, h: Handle) -> Option<usize> {
        let entry = self.load_barrier_slow(ctx, h);
        (entry != 0).then_some((entry & PTR_MASK) as usize)
    }

    fn last_stats(&self) -> &GcStats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::ZgcCollector;
    use crate::runtime_gc::zgc::color::ZColor;

    #[test]
    fn collector_exposes_zgc_color_phase_transitions() {
        let mut collector = ZgcCollector::new();

        assert_eq!(collector.start_mark_for_tests(), ZColor::Marked1);
        assert_eq!(collector.start_relocate_for_tests(), ZColor::Remapped);
        assert_eq!(collector.start_mark_for_tests(), ZColor::Marked0);
    }
}
