//! Sweep phase（spec §8.2，IMPL-7 按 ptr sort 必需）。
//!
//! 核心：**mark-sweep 不移动活动对象**（v2 INV-C1/C2）。用 obj_table + marked bits 按 ptr 顺序重建 free list。
//!
//! 算法：
//! 1. 收集所有已分配块信息（含已死）。依赖 INV-A（obj_table 完整索引）。
//! 2. sort_by_ptr（resize INV-B 破坏 handle→ptr 单调性，sort 必需）。
//! 3. 线性扫描合并相邻 unmarked 块（暂存）。
//! 4. 完整 sweep 游标结束后才合并 abandoned、回收尾部、重建 free list、发布 freed handles。
use crate::runtime_gc::api::GcContext;
use crate::runtime_gc::context::object_size_from_memory;
use crate::runtime_gc::mark_sweep::MarkSweepCollector;

/// 收集的块信息：(ptr, size, handle, is_marked)。
#[derive(Clone, Copy)]
struct BlockInfo {
    ptr: usize,
    size: usize,
    handle: u32,
    marked: bool,
}

/// lazy sweep 过程中的真实游标状态。
pub(crate) struct LazySweepState {
    obj_table_ptr: usize,
    blocks: Vec<BlockInfo>,
    next_block: usize,
    /// 尚未封口的连续 dead run；lazy progress 期间只暂存，不进入 free list/governance。
    pending_free_run: Option<(usize, usize)>,
    /// 已封口的 dead runs；完整游标结束前不得发布为最终 free regions。
    free_runs: Vec<(usize, usize)>,
    swept: usize,
    freed_bytes: usize,
    remaining_bytes: usize,
    started_at: std::time::Instant,
}

impl LazySweepState {
    pub(crate) fn started_at(&self) -> std::time::Instant {
        self.started_at
    }
}

pub(crate) enum LazySweepStep {
    Progress { remaining_estimate: usize },
    Complete,
}

struct LazySweepCompletion {
    free_runs: Vec<(usize, usize)>,
    swept: usize,
    freed_bytes: usize,
}

/// 按 ptr 升序合并物理相邻区间（next.ptr == prev_end）。
fn merge_adjacent_free_intervals(mut intervals: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if intervals.is_empty() {
        return intervals;
    }
    intervals.sort_by_key(|(ptr, _)| *ptr);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    let (mut cur_ptr, mut cur_end) = {
        let (p, s) = intervals[0];
        (p, p.saturating_add(s))
    };
    for &(ptr, size) in intervals.iter().skip(1) {
        let end = ptr.saturating_add(size);
        if ptr == cur_end {
            cur_end = end;
        } else {
            merged.push((cur_ptr, cur_end - cur_ptr));
            cur_ptr = ptr;
            cur_end = end;
        }
    }
    merged.push((cur_ptr, cur_end - cur_ptr));
    merged
}

fn collect_sorted_blocks(
    collector: &MarkSweepCollector,
    ctx: &mut GcContext,
    obj_table_ptr: usize,
) -> Vec<BlockInfo> {
    let count = ctx.obj_table_count();
    let mut blocks: Vec<BlockInfo> = ctx.with_memory(|data| {
        let mut out = Vec::new();
        for h in 0..(count as u32) {
            let addr = obj_table_ptr + h as usize * 4;
            if addr + 4 > data.len() {
                break;
            }
            let ptr =
                u32::from_le_bytes([data[addr], data[addr + 1], data[addr + 2], data[addr + 3]])
                    as usize;
            if ptr == 0 {
                continue; // 空槽（已回收的 handle）
            }
            if ptr < collector.dynamic_start() {
                continue;
            }
            let Some(size) = object_size_from_memory(data, ptr) else {
                continue;
            };
            out.push(BlockInfo {
                ptr,
                size,
                handle: h,
                marked: collector.mark_bits.is_marked(h),
            });
        }
        out
    });
    blocks.sort_by_key(|b| b.ptr);
    blocks
}

fn flush_pending_free_run(
    pending_free_run: &mut Option<(usize, usize)>,
    free_runs: &mut Vec<(usize, usize)>,
) {
    if let Some(run) = pending_free_run.take() {
        free_runs.push(run);
    }
}

fn record_free_run(
    pending_free_run: &mut Option<(usize, usize)>,
    free_runs: &mut Vec<(usize, usize)>,
    ptr: usize,
    size: usize,
) {
    let end = ptr.saturating_add(size);
    match pending_free_run {
        Some((run_ptr, run_size)) if run_ptr.saturating_add(*run_size) == ptr => {
            *run_size = end - *run_ptr;
        }
        Some(_) => {
            flush_pending_free_run(pending_free_run, free_runs);
            *pending_free_run = Some((ptr, size));
        }
        None => {
            *pending_free_run = Some((ptr, size));
        }
    }
}

fn clear_unmarked_handle_slot(data: &mut [u8], obj_table_ptr: usize, handle: u32) {
    let addr = obj_table_ptr + handle as usize * 4;
    if addr + 4 <= data.len() {
        data[addr..addr + 4].copy_from_slice(&0u32.to_le_bytes());
    }
}

fn finalize_free_regions(
    collector: &mut MarkSweepCollector,
    ctx: &mut GcContext,
    sweep_free_runs: Vec<(usize, usize)>,
    swept: usize,
    freed_bytes: usize,
) {
    // 回收 resize-abandoned 区域（P4-blocker #1，INV-B），并与 sweep 空闲区按 ptr 邻接合并（#116）。
    // grow_array/grow_object 重写 obj_table 槽到新 ptr 后，旧 ptr 区域不再被
    // obj_table 索引，块扫描看不到 → 单独注册到 abandoned_regions。
    let abandoned: Vec<(usize, usize)> = ctx.with_state(|st| {
        st.abandoned_regions_for_gc()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default()
    });
    let mut all_free = sweep_free_runs;
    all_free.extend(abandoned);
    let coalesced = merge_adjacent_free_intervals(all_free);

    // 尾部空间回收（issue #332）：仅在完整 sweep 游标结束后执行；
    // 回退 heap_ptr → 减少 linear memory 占用。不移动任何对象（v2 INV-C1/C2 安全）。
    let tail_result =
        crate::runtime_gc::heap_governance::reclaim_trailing_free_space(ctx, &coalesced);

    // 从 free list 移除已被尾部回收的区间（ptr >= new_heap_ptr），再重建。
    let surviving: Vec<(usize, usize)> = coalesced
        .iter()
        .filter(|(ptr, _)| *ptr < tail_result.new_heap_ptr)
        .copied()
        .collect();
    collector
        .free_list
        .rebuild_from_coalesced_regions(&surviving);

    // 碎片指标（issue #332）：完整 sweep 后基于 surviving regions 写入 GcStats。
    let heap_used_bytes = ctx.heap_used();
    let metrics = crate::runtime_gc::heap_governance::compute_metrics(&surviving);
    ctx.stats.free_block_count = metrics.free_block_count;
    ctx.stats.total_free_bytes = metrics.total_free_bytes;
    ctx.stats.largest_free_block = metrics.largest_free_block;
    ctx.stats.external_fragmentation = metrics.external_fragmentation;
    ctx.stats.tail_reclaimed_bytes = tail_result.reclaimed_bytes;
    ctx.stats.heap_used_bytes = heap_used_bytes;
    ctx.stats.swept = swept;
    ctx.stats.freed_bytes = freed_bytes;
}

pub(crate) fn prepare_lazy_sweep(
    collector: &MarkSweepCollector,
    ctx: &mut GcContext,
    started_at: std::time::Instant,
) -> LazySweepState {
    let obj_table_ptr = ctx.obj_table_ptr();
    let blocks = collect_sorted_blocks(collector, ctx, obj_table_ptr);
    let remaining_bytes = blocks.iter().map(|b| b.size).sum();
    LazySweepState {
        obj_table_ptr,
        blocks,
        next_block: 0,
        pending_free_run: None,
        free_runs: Vec::new(),
        swept: 0,
        freed_bytes: 0,
        remaining_bytes,
        started_at,
    }
}

fn sweep_one_block(
    collector: &mut MarkSweepCollector,
    ctx: &mut GcContext,
    state: &mut LazySweepState,
    block: BlockInfo,
) {
    if block.marked {
        flush_pending_free_run(&mut state.pending_free_run, &mut state.free_runs);
        return;
    }

    record_free_run(
        &mut state.pending_free_run,
        &mut state.free_runs,
        block.ptr,
        block.size,
    );
    ctx.with_memory_mut(|data| clear_unmarked_handle_slot(data, state.obj_table_ptr, block.handle));
    collector.freed_handles.push(block.handle);
    state.swept += 1;
    state.freed_bytes += block.size;
}

fn take_lazy_sweep_completion(state: &mut LazySweepState) -> Option<LazySweepCompletion> {
    if state.next_block != state.blocks.len() {
        return None;
    }

    flush_pending_free_run(&mut state.pending_free_run, &mut state.free_runs);
    Some(LazySweepCompletion {
        free_runs: std::mem::take(&mut state.free_runs),
        swept: state.swept,
        freed_bytes: state.freed_bytes,
    })
}

fn complete_lazy_sweep(
    collector: &mut MarkSweepCollector,
    ctx: &mut GcContext,
    state: &mut LazySweepState,
) {
    while state.next_block < state.blocks.len() {
        let block = state.blocks[state.next_block];
        state.next_block += 1;
        state.remaining_bytes = state.remaining_bytes.saturating_sub(block.size);
        sweep_one_block(collector, ctx, state, block);
    }
    let completion = take_lazy_sweep_completion(state)
        .expect("lazy sweep cursor must be exhausted before finalization");
    finalize_free_regions(
        collector,
        ctx,
        completion.free_runs,
        completion.swept,
        completion.freed_bytes,
    );
}

pub(crate) fn sweep_lazy_step(
    collector: &mut MarkSweepCollector,
    ctx: &mut GcContext,
    state: &mut LazySweepState,
    budget_bytes: usize,
    deadline: std::time::Instant,
) -> LazySweepStep {
    let target_bytes = budget_bytes.max(1);
    let mut processed_bytes = 0usize;
    while state.next_block < state.blocks.len()
        && (processed_bytes < target_bytes || processed_bytes == 0)
    {
        if processed_bytes > 0 && std::time::Instant::now() >= deadline {
            break;
        }
        let block = state.blocks[state.next_block];
        state.next_block += 1;
        state.remaining_bytes = state.remaining_bytes.saturating_sub(block.size);
        processed_bytes = processed_bytes.saturating_add(block.size.max(1));
        sweep_one_block(collector, ctx, state, block);
    }

    if let Some(completion) = take_lazy_sweep_completion(state) {
        finalize_free_regions(
            collector,
            ctx,
            completion.free_runs,
            completion.swept,
            completion.freed_bytes,
        );
        LazySweepStep::Complete
    } else {
        LazySweepStep::Progress {
            remaining_estimate: state.remaining_bytes,
        }
    }
}

pub(crate) fn sweep_lazy_to_completion(
    collector: &mut MarkSweepCollector,
    ctx: &mut GcContext,
    state: &mut LazySweepState,
) {
    complete_lazy_sweep(collector, ctx, state);
}

pub fn sweep(collector: &mut MarkSweepCollector, ctx: &mut GcContext) {
    let obj_table_ptr = ctx.obj_table_ptr();
    let blocks = collect_sorted_blocks(collector, ctx, obj_table_ptr);
    let mut pending_free_run = None;
    let mut sweep_free_runs: Vec<(usize, usize)> = Vec::new();
    let mut freed = Vec::new();
    let mut swept = 0usize;
    let mut freed_bytes = 0usize;

    ctx.with_memory_mut(|data| {
        for block in &blocks {
            if block.marked {
                flush_pending_free_run(&mut pending_free_run, &mut sweep_free_runs);
                continue;
            }
            record_free_run(
                &mut pending_free_run,
                &mut sweep_free_runs,
                block.ptr,
                block.size,
            );
            clear_unmarked_handle_slot(data, obj_table_ptr, block.handle);
            freed.push(block.handle);
            swept += 1;
            freed_bytes += block.size;
        }
    });
    flush_pending_free_run(&mut pending_free_run, &mut sweep_free_runs);
    collector.freed_handles.extend(freed);
    finalize_free_regions(collector, ctx, sweep_free_runs, swept, freed_bytes);
}

#[cfg(test)]
mod tests {
    use super::{
        BlockInfo, LazySweepState, merge_adjacent_free_intervals, take_lazy_sweep_completion,
    };
    use crate::runtime_gc::mark_sweep::MarkSweepCollector;

    #[test]
    fn incomplete_lazy_sweep_keeps_final_free_regions_unpublished() {
        let mut state = LazySweepState {
            obj_table_ptr: 0,
            blocks: vec![
                BlockInfo {
                    ptr: 1000,
                    size: 32,
                    handle: 1,
                    marked: false,
                },
                BlockInfo {
                    ptr: 1032,
                    size: 32,
                    handle: 2,
                    marked: false,
                },
            ],
            next_block: 1,
            pending_free_run: Some((1000, 32)),
            free_runs: vec![(900, 16)],
            swept: 1,
            freed_bytes: 32,
            remaining_bytes: 32,
            started_at: std::time::Instant::now(),
        };

        // take_lazy_sweep_completion 是 tail reclaim / free-list rebuild / metrics 的唯一入口；
        // cursor 未完成时必须不给调用方任何最终 free regions。
        assert!(take_lazy_sweep_completion(&mut state).is_none());
        assert_eq!(state.pending_free_run, Some((1000, 32)));
        assert_eq!(state.free_runs, vec![(900, 16)]);
        assert_eq!(state.swept, 1);
        assert_eq!(state.freed_bytes, 32);
    }

    #[test]
    fn completed_lazy_sweep_returns_staged_free_regions_for_finalization() {
        let mut state = LazySweepState {
            obj_table_ptr: 0,
            blocks: vec![BlockInfo {
                ptr: 1000,
                size: 32,
                handle: 1,
                marked: false,
            }],
            next_block: 1,
            pending_free_run: Some((1000, 32)),
            free_runs: vec![(900, 16)],
            swept: 1,
            freed_bytes: 32,
            remaining_bytes: 0,
            started_at: std::time::Instant::now(),
        };

        let completion = take_lazy_sweep_completion(&mut state).expect("cursor complete");

        assert_eq!(completion.free_runs, vec![(900, 16), (1000, 32)]);
        assert_eq!(completion.swept, 1);
        assert_eq!(completion.freed_bytes, 32);
        assert!(state.pending_free_run.is_none());
        assert!(state.free_runs.is_empty());
    }

    /// P4-blocker #1：验证 resize-abandoned 区域经 sweeper 收尾路径进入 free list。
    /// grow_array/grow_object 重写 obj_table 槽后旧 ptr 不可达；这里单独验证
    /// "abandoned (ptr, size) → free_list.add_free_region → alloc 可复用" 的回收链路。
    #[test]
    fn abandoned_region_recovers_into_free_list() {
        let mut collector = MarkSweepCollector::new();
        // 模拟一次 array resize：旧区域 ptr=2000, size=80（16 + 8*8），新区域在更高地址。
        // 块扫描只看到新区域，旧区域经 abandoned 注入 sweep 收尾。
        collector.free_list.add_free_region(2000, 80);
        // 该区域应可被 alloc 复用（best-fit：80 进 class 80，alloc(80) 精确命中）。
        assert_eq!(collector.free_list.alloc(80), Some(2000));
        // 用尽后应 None（验证不是幽灵复用）。
        assert_eq!(collector.free_list.alloc(80), None);
    }

    /// #116：sweep 空闲区与物理相邻的 abandoned 区合并为一块后再入表。
    #[test]
    fn adjacent_abandoned_coalesces_with_sweep_free_run() {
        let merged = merge_adjacent_free_intervals(vec![(1000, 144), (1144, 80)]);
        assert_eq!(merged, vec![(1000, 224)]);
        let mut collector = MarkSweepCollector::new();
        collector.free_list.rebuild_from_coalesced_regions(&merged);
        assert_eq!(collector.free_list.alloc(224), Some(1000));
        assert_eq!(collector.free_list.total_free_regions(), 0);
    }
}
