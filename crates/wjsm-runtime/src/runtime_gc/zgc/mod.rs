#![allow(dead_code)] // T4.1 建立 T4.2-T4.4 会接入的 ZGC collector skeleton。
pub mod color;
mod mark;
pub mod page;

use super::api::{
    AllocRequest, GcAlgorithm, GcContext, GcStats, Handle, RootProvider, StepBudget, StepOutcome,
    Value,
};
use super::mark_sweep::MarkSweepCollector;
use color::{PTR_MASK, ZColor, ZColorState, ZEntry, ZPhase};
use mark::{MarkStep, ZMarkState};
use page::{ZPageSpace, recolor_live_obj_table_entries};
use wasmtime::Val;
use wjsm_ir::constants;

pub(crate) struct ZgcCollector {
    colors: ZColorState,
    pages: ZPageSpace,
    mark: ZMarkState,
    fallback: MarkSweepCollector,
    stats: GcStats,
}

impl ZgcCollector {
    pub(crate) fn new() -> Self {
        Self {
            colors: ZColorState::default(),
            pages: ZPageSpace::default(),
            mark: ZMarkState::new(),
            fallback: MarkSweepCollector::new(),
            stats: GcStats::default(),
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
        let good = self.colors.start_mark();
        self.sync_wasm_color_phase(ctx);
        self.mark.start_cycle(ctx, roots, &mut self.pages, good);
    }

    fn finish_active_mark(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots: &mut dyn RootProvider,
    ) -> Option<GcStats> {
        if !self.mark.is_active() {
            return None;
        }
        let good = self.colors.good();
        let stats = self
            .mark
            .finish_after_barrier_flush(ctx, roots, &mut self.pages, good);
        self.colors.finish_cycle();
        self.sync_wasm_color_phase(ctx);
        self.stats = stats.clone();
        Some(stats)
    }

    fn sync_after_delegate(&mut self, ctx: &mut GcContext<'_>) {
        self.attach_pages_and_recolor(ctx);
        self.stats = self.fallback.last_stats().clone();
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

    fn strip_colors_for_delegate(&mut self, ctx: &mut GcContext<'_>) {
        let obj_table_ptr = ctx.obj_table_ptr();
        let obj_table_count = ctx.obj_table_count();
        let changed = ctx.with_memory_mut(|data| {
            let mut changed = 0usize;
            for handle in 0..obj_table_count {
                let slot = obj_table_ptr + handle * constants::HANDLE_TABLE_ENTRY_SIZE as usize;
                let Some(bytes) = data.get_mut(slot..slot + 4) else {
                    break;
                };
                let mut raw = [0u8; 4];
                raw.copy_from_slice(bytes);
                let entry = ZEntry::from(u32::from_le_bytes(raw));
                if entry.is_empty() || entry.raw() == entry.ptr() {
                    continue;
                }
                bytes.copy_from_slice(&entry.ptr().to_le_bytes());
                changed += 1;
            }
            changed
        });
        if changed != 0 {
            ctx.increment_gc_epoch();
        }
    }

    fn reset_barrier_buffer(ctx: &mut GcContext<'_>) {
        let (_, _, base) = ctx.with_state(|state| state.heap_layout_boundaries());
        if base == 0 {
            return;
        }
        if let Some(global) = ctx.env.barrier_buf_ptr {
            let _ = global.set(&mut ctx.store, Val::I32(base as i32));
        }
    }

    pub(crate) fn start_mark_for_tests(&mut self) -> ZColor {
        self.colors.start_mark()
    }

    pub(crate) fn start_relocate_for_tests(&mut self) -> ZColor {
        self.colors.start_relocate()
    }
}

impl GcAlgorithm for ZgcCollector {
    fn name(&self) -> &'static str {
        "zgc"
    }

    fn attach_heap(&mut self, ctx: &mut GcContext<'_>, dynamic_start: usize) {
        self.fallback.attach_heap(ctx, dynamic_start);
        self.attach_pages_and_recolor(ctx);
    }

    fn alloc_slow(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots: &mut dyn RootProvider,
        req: AllocRequest,
    ) -> Option<usize> {
        let _ = self.finish_active_mark(ctx, roots);
        self.strip_colors_for_delegate(ctx);
        let ptr = self.fallback.alloc_slow(ctx, roots, req);
        self.sync_after_delegate(ctx);
        ptr
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
                    let _ = self.finish_active_mark(ctx, roots);
                    StepOutcome::CycleComplete
                }
            };
        }
        self.start_mark_cycle(ctx, roots);
        StepOutcome::Progress {
            remaining_estimate: budget.work_bytes.max(1),
        }
    }

    fn collect_full(&mut self, ctx: &mut GcContext<'_>, roots: &mut dyn RootProvider) -> GcStats {
        self.refresh_pages(ctx);
        if !self.mark.is_active() {
            self.start_mark_cycle(ctx, roots);
        }
        self.finish_active_mark(ctx, roots)
            .unwrap_or_else(|| self.stats.clone())
    }

    fn load_barrier_slow(&mut self, ctx: &mut GcContext<'_>, h: Handle) -> u32 {
        if self.colors.phase() == ZPhase::Mark {
            return self.mark.mark_from_load_barrier(ctx, h, self.colors.good());
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
