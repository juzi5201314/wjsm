//! MarkSweep 算法（spec §8）：non-moving mark-sweep + segregated free list。
//!
//! 组装：`MarkSweepCollector`（impl GcAlgorithm）持有 `SegregatedFreeList` + `MarkBitmap`。
//! - mark: 显式 worklist（IMPL-6，不递归），移植自 runtime_heap mark_object_recursive。
//! - sweep: 按 ptr sort → 线性合并相邻 unmarked → add_free_region（IMPL-7）。
pub mod allocator;
pub mod marker;
pub mod sweeper;

use crate::runtime_gc::api::{
    Allocator, GcAlgorithm, GcContext, GcStats, Handle, Marker, RootProvider, Sweeper,
};
use crate::runtime_gc::mark_bitmap::MarkBitmap;
use crate::runtime_gc::weak_refs;
use allocator::SegregatedFreeList;

/// non-moving mark-sweep 收集器。
pub struct MarkSweepCollector {
    pub(crate) free_list: SegregatedFreeList,
    pub(crate) mark_bits: MarkBitmap,
    /// sweep 回收的 handle 槽（供 fast-path 复用，IMPL-10/#7）。
    pub(crate) freed_handles: Vec<Handle>,
}

impl MarkSweepCollector {
    pub fn new() -> Self {
        Self {
            free_list: SegregatedFreeList::new(),
            mark_bits: MarkBitmap::new(),
            freed_handles: Vec::new(),
        }
    }
}

impl Default for MarkSweepCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl Allocator for MarkSweepCollector {
    fn alloc_slow(
        &mut self,
        ctx: &mut GcContext,
        size: usize,
        _heap_type: u8,
        _capacity: u32,
    ) -> Option<usize> {
        // 1. 试 free list best-fit
        if let Some(ptr) = self.free_list.alloc(size) {
            return Some(ptr);
        }
        // 2. 试 bump（heap_ptr 推进）
        let heap_ptr = ctx.heap_ptr();
        let mem_end = ctx.env.memory.data_size(&ctx.store);
        if heap_ptr + size <= mem_end {
            ctx.set_heap_ptr(heap_ptr + size);
            return Some(heap_ptr);
        }
        None
    }

    fn add_free_region(&mut self, ptr: usize, size: usize) {
        self.free_list.add_free_region(ptr, size);
    }
}

impl Marker for MarkSweepCollector {
    fn mark(&mut self, ctx: &mut GcContext, roots: &mut dyn Iterator<Item = Handle>) {
        marker::mark_roots_and_drain(self, ctx, roots);
    }

    fn is_marked(&self, h: Handle) -> bool {
        self.mark_bits.is_marked(h)
    }
}

impl Sweeper for MarkSweepCollector {
    fn sweep(&mut self, ctx: &mut GcContext) {
        sweeper::sweep(self, ctx);
    }
}

impl GcAlgorithm for MarkSweepCollector {
    fn collect(&mut self, ctx: &mut GcContext) -> GcStats {
        let start = std::time::Instant::now();
        // 1. reset mark bits（容量 = obj_table_count）
        let count = ctx.obj_table_count();
        self.mark_bits.reset(count);
        self.freed_handles.clear();

        // 2. mark phase：roots（RootProvider 由宿主经 mark 的 roots 迭代器注入）
        //    实际 root 收集由 P4 集成时由宿主把 roots 喂给 mark。
        //    collect 入口接收一个空迭代器占位；真实 root 集经专门方法注入。
        //    见 collect_with_roots。
        let empty: std::iter::Empty<Handle> = std::iter::empty();
        self.mark(ctx, &mut Box::new(empty) as _);

        // 3. sweep phase
        self.sweep(ctx);

        // 4. 把 freed_handles 推入 RuntimeState.handle_free_list（P4 接管 fast-path 复用）
        weak_refs::process_weak_refs_after_sweep(ctx, &self.freed_handles);
        weak_refs::cleanup_stream_tables_after_sweep(ctx, &self.freed_handles);
        ctx.with_state(|st| {
            if let Some(mut list) = st.handle_free_list_for_gc() {
                list.extend_from_slice(&self.freed_handles);
            }
        });

        ctx.stats.elapsed = start.elapsed();
        ctx.stats.clone()
    }

    fn algorithm_name(&self) -> &'static str {
        "mark-sweep"
    }

    /// 带 RootProvider 的完整 collect，含 fixed-point host-table root 追踪（IMPL-9，spec §10）。
    ///
    /// 流程：
    /// 1. mark shadow stack + function property roots → drain
    /// 2. loop：mark host-table roots（microtask/promise/continuation/streams/...）→ drain
    ///    → 若 popcount 增长则重复（promise reactions 引用经 mark 可达后，reaction target 需再标）
    ///    → until popcount 不变（fixed-point 收敛，受 host 表大小约束）
    /// 3. sweep → free list + abandoned regions + handle 复用
    fn collect_with_provider(
        &mut self,
        ctx: &mut GcContext,
        roots: &mut dyn RootProvider,
    ) -> GcStats {
        let start = std::time::Instant::now();
        let count = ctx.obj_table_count();
        self.mark_bits.reset(count);
        self.freed_handles.clear();

        // 1. shadow stack + function property roots（稳定 root）
        let shadow_roots: Vec<Handle> = {
            let mut acc = Vec::new();
            roots.for_each_shadow_stack_root(ctx, &mut |h| acc.push(h));
            acc
        };
        self.mark(ctx, &mut shadow_roots.into_iter());

        // 2. fixed-point：host-table roots 多轮注入 until popcount 不变
        loop {
            let before = self.mark_bits.popcount();
            let host_roots: Vec<Handle> = {
                let mut acc = Vec::new();
                roots.for_each_host_table_root(ctx, &mut |h| acc.push(h));
                acc
            };
            self.mark(ctx, &mut host_roots.into_iter());
            let after = self.mark_bits.popcount();
            if after == before {
                break;
            }
        }

        // 3. sweep
        self.sweep(ctx);
        weak_refs::process_weak_refs_after_sweep(ctx, &self.freed_handles);
        weak_refs::cleanup_stream_tables_after_sweep(ctx, &self.freed_handles);
        ctx.with_state(|st| {
            if let Some(mut list) = st.handle_free_list_for_gc() {
                list.extend_from_slice(&self.freed_handles);
            }
        });

        ctx.stats.marked = self.mark_bits.popcount();
        ctx.stats.elapsed = start.elapsed();
        ctx.stats.clone()
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
        let start = std::time::Instant::now();
        let count = ctx.obj_table_count();
        self.mark_bits.reset(count);
        self.freed_handles.clear();

        self.mark(ctx, roots);
        self.sweep(ctx);

        weak_refs::process_weak_refs_after_sweep(ctx, &self.freed_handles);
        weak_refs::cleanup_stream_tables_after_sweep(ctx, &self.freed_handles);
        ctx.with_state(|st| {
            if let Some(mut list) = st.handle_free_list_for_gc() {
                list.extend_from_slice(&self.freed_handles);
            }
        });

        ctx.stats.elapsed = start.elapsed();
        ctx.stats.clone()
    }
}
