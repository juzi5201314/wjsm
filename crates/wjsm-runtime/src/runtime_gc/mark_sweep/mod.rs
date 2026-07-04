//! MarkSweep 算法（spec §8）：non-moving mark-sweep + segregated free list。
//!
//! 组装：`MarkSweepCollector`（impl GcAlgorithm）持有 `SegregatedFreeList` + `MarkBitmap`。
//! - mark: 显式 worklist（IMPL-6，不递归），移植自 runtime_heap mark_object_recursive。
//! - sweep: 按 ptr sort → 线性合并相邻 unmarked → add_free_region（IMPL-7）。
pub mod allocator;
pub mod marker;
pub mod sweeper;

use crate::runtime_gc::api::{
    AllocRequest, GcAlgorithm, GcContext, GcStats, Handle, RootProvider, StepBudget, StepOutcome,
};
use crate::runtime_gc::mark_bitmap::MarkBitmap;
use crate::runtime_gc::weak_refs;
use allocator::SegregatedFreeList;

/// non-moving mark-sweep 收集器。
pub struct MarkSweepCollector {
    pub(crate) free_list: SegregatedFreeList,
    pub(crate) mark_bits: MarkBitmap,
    /// 本周期 sweep 回收的 handle 暂存；finalize_sweep_cycle 才发布给 fast-path 复用。
    pub(crate) freed_handles: Vec<Handle>,
    /// v2 接管的动态堆起点；MarkSweep 的动态域仍是连续 bump 区间。
    dynamic_start: usize,
    /// 最近一次完成的 v2/v1 GC 周期统计。
    stats_cache: GcStats,
    /// lazy sweep 的真实块游标；None 表示当前没有增量 sweep 工作。
    lazy_sweep: Option<sweeper::LazySweepState>,
}

impl MarkSweepCollector {
    pub fn new() -> Self {
        Self {
            free_list: SegregatedFreeList::new(),
            mark_bits: MarkBitmap::new(),
            freed_handles: Vec::new(),
            dynamic_start: 0,
            stats_cache: GcStats::default(),
            lazy_sweep: None,
        }
    }

    pub(crate) fn dynamic_start(&self) -> usize {
        self.dynamic_start
    }

    fn reclaim_owner_backed_side_tables(&self, ctx: &mut GcContext) {
        ctx.with_state(|st| {
            st.reclaim_unmarked_collection_entries(|h| self.mark_bits.is_marked(h));
        });
    }

    fn sync_alloc_window(&self, ctx: &mut GcContext) {
        let heap_ptr = ctx.heap_ptr();
        let mem_end = ctx.env.memory.data_size(&ctx.store);
        let alloc_end = mem_end.min(ctx.heap_limit());
        ctx.alloc_window_set(heap_ptr, alloc_end);
    }

    fn alloc_from_free_list(&mut self, size: usize) -> Option<usize> {
        self.free_list.alloc(size)
    }

    fn alloc_from_bump_window(&mut self, ctx: &mut GcContext, size: usize) -> Option<usize> {
        let heap_ptr = ctx.heap_ptr();
        if heap_ptr < self.dynamic_start {
            self.sync_alloc_window(ctx);
            return None;
        }
        let Some(new_heap_ptr) = heap_ptr.checked_add(size) else {
            self.sync_alloc_window(ctx);
            return None;
        };
        let mem_end = ctx.env.memory.data_size(&ctx.store);
        let alloc_end = mem_end.min(ctx.heap_limit());
        if new_heap_ptr <= alloc_end {
            ctx.set_heap_ptr(new_heap_ptr);
            self.sync_alloc_window(ctx);
            Some(heap_ptr)
        } else {
            self.sync_alloc_window(ctx);
            None
        }
    }

    fn begin_mark_cycle(&mut self, ctx: &mut GcContext) {
        ctx.stats = GcStats::default();
        let count = ctx.obj_table_count();
        self.mark_bits.reset(count);
        self.freed_handles.clear();
    }

    fn mark_provider_fixed_point(&mut self, ctx: &mut GcContext, roots: &mut dyn RootProvider) {
        self.begin_mark_cycle(ctx);

        // 1. shadow stack + function property roots（稳定 root）
        let shadow_roots: Vec<Handle> = {
            let mut acc = Vec::new();
            roots.for_each_shadow_stack_root(ctx, &mut |h| acc.push(h));
            acc
        };
        marker::mark_roots_and_drain(self, ctx, &mut shadow_roots.into_iter());

        // 2. fixed-point：host-table roots 多轮注入 until popcount 不变。
        loop {
            let before = self.mark_bits.popcount();
            let host_roots: Vec<Handle> = {
                let mut acc = Vec::new();
                let mut is_marked = |h| self.mark_bits.is_marked(h);
                roots.for_each_host_table_root(ctx, &mut is_marked, &mut |h| acc.push(h));
                acc
            };
            marker::mark_roots_and_drain(self, ctx, &mut host_roots.into_iter());
            let after = self.mark_bits.popcount();
            if after == before {
                break;
            }
        }
    }

    fn finalize_sweep_cycle(
        &mut self,
        ctx: &mut GcContext,
        started_at: std::time::Instant,
    ) -> GcStats {
        self.reclaim_owner_backed_side_tables(ctx);
        weak_refs::process_weak_refs_after_sweep(ctx, &self.freed_handles);
        weak_refs::cleanup_stream_tables_after_sweep(ctx, &self.freed_handles);
        ctx.with_state(|st| {
            if let Some(mut list) = st.handle_free_list_for_gc() {
                list.extend_from_slice(&self.freed_handles);
            }
        });

        ctx.increment_gc_epoch();
        ctx.stats.marked = self.mark_bits.popcount();
        ctx.stats.elapsed = started_at.elapsed();
        let stats = ctx.stats.clone();
        self.stats_cache = stats.clone();
        self.sync_alloc_window(ctx);
        stats
    }

    fn sweep_and_finalize(
        &mut self,
        ctx: &mut GcContext,
        started_at: std::time::Instant,
    ) -> GcStats {
        sweeper::sweep(self, ctx);
        self.finalize_sweep_cycle(ctx, started_at)
    }

    fn finish_pending_lazy_sweep(&mut self, ctx: &mut GcContext) -> Option<GcStats> {
        let mut state = self.lazy_sweep.take()?;
        let started_at = state.started_at();
        sweeper::sweep_lazy_to_completion(self, ctx, &mut state);
        Some(self.finalize_sweep_cycle(ctx, started_at))
    }

    fn advance_lazy_sweep(
        &mut self,
        ctx: &mut GcContext,
        work_bytes: usize,
        deadline: std::time::Instant,
    ) -> StepOutcome {
        let Some(mut state) = self.lazy_sweep.take() else {
            return StepOutcome::Idle;
        };
        let started_at = state.started_at();
        match sweeper::sweep_lazy_step(self, ctx, &mut state, work_bytes, deadline) {
            sweeper::LazySweepStep::Progress { remaining_estimate } => {
                ctx.increment_gc_epoch();
                self.lazy_sweep = Some(state);
                StepOutcome::Progress { remaining_estimate }
            }
            sweeper::LazySweepStep::Complete => {
                self.finalize_sweep_cycle(ctx, started_at);
                StepOutcome::CycleComplete
            }
        }
    }
}

impl Default for MarkSweepCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl GcAlgorithm for MarkSweepCollector {
    fn name(&self) -> &'static str {
        "mark-sweep"
    }

    fn attach_heap(&mut self, ctx: &mut GcContext, dynamic_start: usize) {
        self.dynamic_start = dynamic_start;
        self.sync_alloc_window(ctx);
    }

    fn alloc_slow(
        &mut self,
        ctx: &mut GcContext,
        roots: &mut dyn RootProvider,
        req: AllocRequest,
    ) -> Option<usize> {
        if let Some(ptr) = self.alloc_from_free_list(req.size) {
            return Some(ptr);
        }

        if self.lazy_sweep.is_some() {
            let _ = self.advance_lazy_sweep(ctx, req.size.max(1), std::time::Instant::now());
            if let Some(ptr) = self.alloc_from_free_list(req.size) {
                return Some(ptr);
            }
        }

        if let Some(ptr) = self.alloc_from_bump_window(ctx, req.size) {
            return Some(ptr);
        }

        let _stats = self.collect_full(ctx, roots);
        if let Some(ptr) = self.alloc_from_free_list(req.size) {
            return Some(ptr);
        }
        if let Some(ptr) = self.alloc_from_bump_window(ctx, req.size) {
            return Some(ptr);
        }

        if matches!(ctx.grow_to_fit_heap_allocation(req.size), Ok(true)) {
            self.sync_alloc_window(ctx);
            return self.alloc_from_bump_window(ctx, req.size);
        }
        self.sync_alloc_window(ctx);
        None
    }

    fn safepoint_step(
        &mut self,
        ctx: &mut GcContext,
        roots: &mut dyn RootProvider,
        budget: StepBudget,
    ) -> StepOutcome {
        if self.lazy_sweep.is_none() {
            let started_at = std::time::Instant::now();
            self.mark_provider_fixed_point(ctx, roots);
            self.lazy_sweep = Some(sweeper::prepare_lazy_sweep(self, ctx, started_at));
        }
        self.advance_lazy_sweep(ctx, budget.work_bytes, budget.deadline)
    }

    fn collect_full(&mut self, ctx: &mut GcContext, roots: &mut dyn RootProvider) -> GcStats {
        let _ = self.finish_pending_lazy_sweep(ctx);

        let started_at = std::time::Instant::now();
        self.mark_provider_fixed_point(ctx, roots);
        self.sweep_and_finalize(ctx, started_at)
    }

    fn last_stats(&self) -> &GcStats {
        &self.stats_cache
    }
}

impl MarkSweepCollector {
    /// 带 root 迭代器的完整 collect（P4 集成时宿主调用此方法而非 collect）。
    #[allow(dead_code)]
    pub fn collect_with_roots(
        &mut self,
        ctx: &mut GcContext,
        roots: &mut dyn Iterator<Item = Handle>,
    ) -> GcStats {
        let _ = self.finish_pending_lazy_sweep(ctx);
        let started_at = std::time::Instant::now();
        marker::mark_roots_and_drain(self, ctx, roots);
        self.sweep_and_finalize(ctx, started_at)
    }
}
