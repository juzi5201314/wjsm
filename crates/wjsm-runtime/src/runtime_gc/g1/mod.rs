mod region;

use super::api::{
    AllocRequest, GcAlgorithm, GcContext, GcStats, RootProvider, StepBudget, StepOutcome,
};
use super::mark_sweep::MarkSweepCollector;
use region::{REGION_SIZE, RegionKind, RegionSpace};

pub(crate) struct G1Collector {
    regions: RegionSpace,
    fallback: MarkSweepCollector,
    stats: GcStats,
}

impl G1Collector {
    pub(crate) fn new() -> Self {
        Self {
            regions: RegionSpace::default(),
            fallback: MarkSweepCollector::new(),
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

    fn install_next_eden_window(&mut self, ctx: &mut GcContext<'_>) -> bool {
        let Some(region_idx) = self.regions.take_free_as(RegionKind::Eden) else {
            return false;
        };
        let Some(start) = self.regions.region_start(region_idx) else {
            return false;
        };
        ctx.set_heap_ptr(start);
        ctx.alloc_window_set(start, start + REGION_SIZE);
        true
    }

    fn sync_after_delegate(&mut self, ctx: &mut GcContext<'_>) {
        self.refresh_regions(ctx);
        self.stats = self.fallback.last_stats().clone();
    }
}

impl GcAlgorithm for G1Collector {
    fn name(&self) -> &'static str {
        "g1"
    }

    fn attach_heap(&mut self, ctx: &mut GcContext<'_>, dynamic_start: usize) {
        self.fallback.attach_heap(ctx, dynamic_start);
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
        if req.size <= REGION_SIZE / 2 && self.install_next_eden_window(ctx) {
            return self.fallback.alloc_slow(ctx, roots, req);
        }
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
        self.refresh_regions(ctx);
        self.stats = stats.clone();
        stats
    }

    fn last_stats(&self) -> &GcStats {
        &self.stats
    }
}
