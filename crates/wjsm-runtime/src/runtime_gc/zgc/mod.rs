#![allow(dead_code)] // T4.1 建立 T4.2-T4.4 会接入的 ZGC collector skeleton。
pub mod color;
pub mod page;

use super::api::{
    AllocRequest, GcAlgorithm, GcContext, GcStats, RootProvider, StepBudget, StepOutcome,
};
use super::mark_sweep::MarkSweepCollector;
use color::{ZColor, ZColorState};
use page::{ZPageSpace, recolor_live_obj_table_entries};

pub(crate) struct ZgcCollector {
    colors: ZColorState,
    pages: ZPageSpace,
    fallback: MarkSweepCollector,
    stats: GcStats,
}

impl ZgcCollector {
    pub(crate) fn new() -> Self {
        Self {
            colors: ZColorState::default(),
            pages: ZPageSpace::default(),
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
        ctx.with_memory_mut(|data| {
            recolor_live_obj_table_entries(data, obj_table_ptr, obj_table_count, good);
        });
    }

    fn sync_after_delegate(&mut self, ctx: &mut GcContext<'_>) {
        self.pages
            .extend_for_committed_end(Self::committed_end(ctx));
        self.stats = self.fallback.last_stats().clone();
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
        let outcome = self.fallback.safepoint_step(ctx, roots, budget);
        self.sync_after_delegate(ctx);
        outcome
    }

    fn collect_full(&mut self, ctx: &mut GcContext<'_>, roots: &mut dyn RootProvider) -> GcStats {
        let stats = self.fallback.collect_full(ctx, roots);
        self.pages
            .extend_for_committed_end(Self::committed_end(ctx));
        self.stats = stats.clone();
        stats
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

        assert_eq!(collector.start_mark_for_tests(), ZColor::Marked0);
        assert_eq!(collector.start_relocate_for_tests(), ZColor::Remapped);
        assert_eq!(collector.start_mark_for_tests(), ZColor::Marked1);
    }
}
