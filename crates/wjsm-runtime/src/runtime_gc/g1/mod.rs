mod concurrent_mark;
mod mixed;
mod region;
mod rset;
mod young;

use super::api::{
    AllocRequest, CycleKind, GcAlgorithm, GcContext, GcStats, Handle, RootProvider, StepBudget,
    StepOutcome, Value,
};
use crate::runtime_gc::context::object_size_from_memory;
use concurrent_mark::{ConcurrentMark, MarkStep};
use region::RegionSpace;
use rset::{BarrierEvent, G1RSet, SlotOwner};
use wasmtime::Val;
use wjsm_ir::constants;

pub(crate) struct G1Collector {
    regions: RegionSpace,
    rset: G1RSet,
    mark: ConcurrentMark,
    mixed_pending: bool,
    stats: GcStats,
}

impl G1Collector {
    pub(crate) fn new() -> Self {
        Self {
            regions: RegionSpace::default(),
            rset: G1RSet::default(),
            mark: ConcurrentMark::new(),
            mixed_pending: false,
            stats: GcStats::default(),
        }
    }

    fn committed_end(ctx: &mut GcContext<'_>) -> usize {
        ctx.heap_limit().min(ctx.env.memory.data_size(&ctx.store))
    }

    fn refresh_regions(&mut self, ctx: &mut GcContext<'_>) {
        let (immortal_end, dynamic_start, _) =
            ctx.with_state(|state| state.heap_layout_boundaries());
        self.regions.attach(
            ctx.object_heap_start(),
            immortal_end,
            dynamic_start,
            Self::committed_end(ctx),
        );
    }

    fn owner_for_handle(&self, ctx: &mut GcContext<'_>, h: Handle) -> Option<SlotOwner> {
        let base = ctx.obj_table_ptr();
        let ptr = ctx.with_memory(|data| {
            let slot =
                base.checked_add(h as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize)?;
            let bytes: [u8; 4] = data.get(slot..slot + 4)?.try_into().ok()?;
            let entry = u32::from_le_bytes(bytes);
            (entry != 0).then_some(entry as usize)
        })?;
        self.owner_for_ptr(ptr)
    }

    fn owner_for_value(&self, ctx: &mut GcContext<'_>, val: Value) -> Option<SlotOwner> {
        rset::value_to_handle(val).and_then(|h| self.owner_for_handle(ctx, h))
    }

    fn owner_for_ptr(&self, ptr: usize) -> Option<SlotOwner> {
        let region_idx = self.regions.region_index(ptr)?;
        Some(SlotOwner {
            region_idx,
            kind: self.regions.kind(region_idx)?,
        })
    }

    fn owner_for_slot(&self, ctx: &mut GcContext<'_>, slot_addr: usize) -> Option<SlotOwner> {
        let base = ctx.obj_table_ptr();
        let count = ctx.obj_table_count();
        let result = ctx.with_memory(|data| {
            for h in 0..count {
                let slot = base + h * constants::HANDLE_TABLE_ENTRY_SIZE as usize;
                let bytes: [u8; 4] = data.get(slot..slot + 4)?.try_into().ok()?;
                let ptr = u32::from_le_bytes(bytes) as usize;
                if ptr == 0 {
                    continue;
                }
                let Some(size) = object_size_from_memory(data, ptr) else {
                    continue;
                };
                if slot_addr >= ptr && slot_addr < ptr.saturating_add(size) {
                    return Some(ptr);
                }
            }
            Some(0)
        })?;
        (result != 0).then(|| self.owner_for_ptr(result)).flatten()
    }

    fn record_barrier_write(
        &mut self,
        ctx: &mut GcContext<'_>,
        target: Handle,
        slot_addr: usize,
        old_val: Value,
        new_val: Value,
    ) {
        let Some(owner) = self.owner_for_handle(ctx, target) else {
            return;
        };
        let Some(card_idx) = self.regions.card_index(slot_addr) else {
            return;
        };
        let new_owner = self.owner_for_value(ctx, new_val);
        self.rset
            .record_write(slot_addr, old_val, new_val, owner, card_idx, new_owner);
    }

    fn barrier_buffer_range(&mut self, ctx: &mut GcContext<'_>) -> Option<(usize, usize)> {
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

    fn merge_stats(primary: &mut GcStats, extra: &GcStats) {
        primary.merge_from(extra);
    }

    fn mixed_step_outcome(result: &mixed::MixedCollection) -> StepOutcome {
        if result.has_remaining() {
            StepOutcome::Progress {
                remaining_estimate: result.remaining_estimate(),
            }
        } else if result.did_work() {
            StepOutcome::CycleComplete
        } else {
            StepOutcome::Idle
        }
    }

    fn run_mixed_step(&mut self, ctx: &mut GcContext<'_>, budget_bytes: usize) -> StepOutcome {
        let result = mixed::collect_step(ctx, &mut self.regions, &mut self.rset, budget_bytes);
        self.mixed_pending = result.has_remaining();
        self.stats = result.stats.clone();
        Self::mixed_step_outcome(&result)
    }
}

impl GcAlgorithm for G1Collector {
    fn name(&self) -> &'static str {
        "g1"
    }

    fn attach_heap(&mut self, ctx: &mut GcContext<'_>, _dynamic_start: usize) {
        self.refresh_regions(ctx);
        if let Some((start, end)) = self.regions.eden_window() {
            ctx.set_heap_ptr(start);
            ctx.alloc_window_set(start, end);
        }
    }

    fn alloc_slow(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots: &mut dyn RootProvider,
        req: AllocRequest,
    ) -> Option<usize> {
        self.refresh_regions(ctx);
        self.barrier_flush(ctx);
        let mut ptr = young::alloc_slow(
            ctx,
            roots,
            req,
            &mut self.regions,
            &mut self.rset,
            self.mark.active_epoch(),
        );
        if ptr.is_none() && self.mixed_pending {
            let _ = self.run_mixed_step(ctx, usize::MAX);
            ptr = young::alloc_slow(
                ctx,
                roots,
                req,
                &mut self.regions,
                &mut self.rset,
                self.mark.active_epoch(),
            );
        }
        self.stats = ctx.stats.clone();
        ptr
    }

    fn safepoint_step(
        &mut self,
        ctx: &mut GcContext<'_>,
        roots: &mut dyn RootProvider,
        budget: StepBudget,
    ) -> StepOutcome {
        self.refresh_regions(ctx);
        self.barrier_flush(ctx);
        if self.mark.is_active() {
            return match self.mark.drain_incremental(ctx, &mut self.rset, budget) {
                MarkStep::Progress { remaining_estimate } => {
                    StepOutcome::Progress { remaining_estimate }
                }
                MarkStep::ReadyForRemark => {
                    self.barrier_flush(ctx);
                    let mut stats = self.mark.finish_after_barrier_flush(
                        ctx,
                        roots,
                        &mut self.regions,
                        &mut self.rset,
                    );
                    let mixed = mixed::collect_step(
                        ctx,
                        &mut self.regions,
                        &mut self.rset,
                        budget.work_bytes,
                    );
                    Self::merge_stats(&mut stats, &mixed.stats);
                    self.mixed_pending = mixed.has_remaining();
                    self.stats = stats.clone();
                    ctx.stats = stats;
                    Self::mixed_step_outcome(&mixed)
                }
            };
        }
        if self.mixed_pending {
            return self.run_mixed_step(ctx, budget.work_bytes);
        }
        if self.mark.should_start(ctx, &self.regions) {
            let _ = young::collect_young(ctx, roots, &mut self.regions, &mut self.rset, None);
            self.refresh_regions(ctx);
            self.mark.start_cycle(ctx, &mut self.rset);
            self.mark
                .initial_mark(ctx, roots, &self.regions, &mut self.rset);
            StepOutcome::Progress {
                remaining_estimate: budget.work_bytes.max(1),
            }
        } else {
            StepOutcome::Idle
        }
    }

    fn collect_full(&mut self, ctx: &mut GcContext<'_>, roots: &mut dyn RootProvider) -> GcStats {
        self.barrier_flush(ctx);
        self.refresh_regions(ctx);
        let mut stats = young::collect_young(ctx, roots, &mut self.regions, &mut self.rset, None);
        self.refresh_regions(ctx);
        self.mark.start_cycle(ctx, &mut self.rset);
        self.mark
            .initial_mark(ctx, roots, &self.regions, &mut self.rset);
        self.barrier_flush(ctx);
        let mark_stats =
            self.mark
                .finish_after_barrier_flush(ctx, roots, &mut self.regions, &mut self.rset);
        Self::merge_stats(&mut stats, &mark_stats);
        loop {
            let mixed = mixed::collect_step(ctx, &mut self.regions, &mut self.rset, usize::MAX);
            Self::merge_stats(&mut stats, &mixed.stats);
            self.mixed_pending = mixed.has_remaining();
            if !mixed.did_work() || !self.mixed_pending {
                break;
            }
        }
        stats.cycle_kind = CycleKind::Full;
        self.stats = stats.clone();
        ctx.stats = stats.clone();
        stats
    }

    fn barrier_flush(&mut self, ctx: &mut GcContext<'_>) {
        let Some((base, ptr)) = self.barrier_buffer_range(ctx) else {
            return;
        };
        if ptr <= base {
            return;
        }
        let events = ctx.with_memory(|data| {
            data.get(base..ptr)
                .map(|buf| rset::decode_buffer(buf).collect::<Vec<BarrierEvent>>())
                .unwrap_or_default()
        });
        for event in events {
            let Some(owner) = self.owner_for_slot(ctx, event.slot_addr as usize) else {
                continue;
            };
            let Some(card_idx) = self.regions.card_index(event.slot_addr as usize) else {
                continue;
            };
            let new_owner = self.owner_for_value(ctx, event.new_value);
            self.rset.record_event(event, owner, card_idx, new_owner);
        }
        Self::reset_barrier_buffer(ctx, base);
    }

    fn on_host_write(
        &mut self,
        ctx: &mut GcContext<'_>,
        target: Handle,
        slot_addr: usize,
        old_val: Value,
        new_val: Value,
    ) {
        self.record_barrier_write(ctx, target, slot_addr, old_val, new_val);
    }

    fn last_stats(&self) -> &GcStats {
        &self.stats
    }
}
