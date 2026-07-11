//! 堆碎片治理（issue #332）。
//!
//! ## 背景
//!
//! non-moving mark-sweep + segregated free list 在长期 churn 下会累积外部碎片：
//! 大量小对象分配-释放后，free list 中散落着许多无法满足大块请求的小碎片，
//! 而 heap_ptr（bump 指针）只增不减，linear memory 页永不释放。
//!
//! ## 约束
//!
//! **mark-sweep 局部 non-moving**：当前治理只服务 mark-sweep，遵守 v2
//! **INV-C1/C2**——对象地址在本算法生命周期内不被本模块移动；潜在 GC 点
//! 之后 raw ptr 必须重新 resolve，不能把尾部回收等同于对象搬迁。
//! 故本模块只降低全空闲尾部的 heap_ptr，**不移动对象**。
//!
//! ## 策略
//!
//! 采用 **non-moving region governance**：
//!
//! 1. **尾部空间回收（trailing free space reclamation）**：
//!    sweep 后若堆顶（高地址端）存在连续空闲区，将 heap_ptr 回退到该区间起始处。
//!    这不会移动任何对象——只是回收从未被分配或已被 sweep 释放的尾部空间。
//!    回收后该区间从 free list 移除，heap_ptr 降低 → 减少 linear memory 占用。
//!
//! 2. **地址序优先分配（address-ordered best-fit）**：
//!    分配时优先使用低地址的 free block（SegregatedFreeList::alloc 已是 best-fit，
//!    但 VecDeque 是 FIFO 而非地址序）。在 sweep 重建 free list 时按 ptr 升序入表，
//!    使 alloc 自然倾向低地址 → 活动对象向堆底压缩 → 最大化尾部连续空闲。
//!
//! 3. **碎片指标（fragmentation metrics）**：
//!    sweep 后计算 free block 数、最大连续空闲块、外部碎片率，暴露给 GcStats。
//!
//! ## 不变量
//!
//! - **TRAIL-1**：heap_ptr 只能在完整 sweep 游标结束后、且尾部全空闲时降低。
//! - **TRAIL-2**：降低 heap_ptr 前，调用方必须丢弃所有 ptr >= new_heap_ptr 的空闲区间。
//! - **TRAIL-3**：abandoned_regions 只在 sweep 完成收尾时 drain，lazy progress 不发布最终 free regions。
//! - **TRAIL-4**：heap_ptr 不得低于 object_heap_start（堆基址）。

use crate::runtime_gc::api::GcContext;

/// 尾部空间回收结果。
#[derive(Debug, Clone, Default)]
pub struct TailReclaimResult {
    /// 回收后的 heap_ptr（< 回收前表示成功回收）。
    pub new_heap_ptr: usize,
    /// 回收的字节数。
    pub reclaimed_bytes: usize,
}

/// sweep 后执行尾部空间回收。
///
/// 算法：
/// 1. 接收 sweeper 在完整 sweep 结束后合并的空闲区间（sweep + abandoned）。
/// 2. 找到最高地址的连续空闲区间末尾。
/// 3. 若该末尾 == heap_ptr，则 heap_ptr 回退到该区间起始。
/// 4. 返回 new_heap_ptr；调用方据此过滤 free list 与碎片指标输入。
///
/// 安全性：
/// - 只回收**物理上连续到 heap_ptr**的尾部空闲区——中间有活对象则不回退。
/// - 不移动任何对象，不修改 obj_table。
/// - heap_ptr 降低后，WASM fast-path bump 从新位置开始，不会覆盖活对象。
pub fn reclaim_trailing_free_space(
    ctx: &mut GcContext,
    free_regions: &[(usize, usize)],
) -> TailReclaimResult {
    let heap_ptr = ctx.heap_ptr();
    let heap_start = ctx.object_heap_start();

    if heap_ptr <= heap_start || free_regions.is_empty() {
        return TailReclaimResult {
            new_heap_ptr: heap_ptr,
            reclaimed_bytes: 0,
        };
    }

    // 找到末尾恰好接到 heap_ptr 的连续空闲区间。
    // free_regions 已由 sweeper 的 merge_adjacent_free_intervals 合并过，
    // 但可能有多个不连续的区间。从 heap_ptr 往回找连续的尾部空闲。
    let mut tail_start = heap_ptr;
    let mut sorted: Vec<(usize, usize)> = free_regions.to_vec();
    sorted.sort_by_key(|(ptr, _)| *ptr);

    // 从高地址向低地址扫描：找到所有末尾 == tail_start 的区间，合并为更大的尾部空闲。
    // 只需找一段恰好接到当前 tail_start 的；合并后继续往前找。
    // 由于已排序，从后往前遍历。
    loop {
        let mut found = false;
        for &(ptr, size) in sorted.iter().rev() {
            let end = ptr + size;
            if end == tail_start && ptr >= heap_start {
                tail_start = ptr;
                found = true;
                break;
            }
        }
        if !found {
            break;
        }
    }

    if tail_start >= heap_ptr {
        // 没有可回收的尾部空间
        return TailReclaimResult {
            new_heap_ptr: heap_ptr,
            reclaimed_bytes: 0,
        };
    }

    let reclaimed = heap_ptr - tail_start;

    // 降低 heap_ptr
    ctx.set_heap_ptr(tail_start);

    TailReclaimResult {
        new_heap_ptr: tail_start,
        reclaimed_bytes: reclaimed,
    }
}

/// 碎片指标（从 free region 列表派生）。
#[derive(Debug, Clone, Default)]
pub struct FragmentationMetrics {
    /// 空闲块总数。
    pub free_block_count: usize,
    /// 总空闲字节数。
    pub total_free_bytes: usize,
    /// 最大连续空闲块字节数。
    pub largest_free_block: usize,
    /// 外部碎片率：1 - (largest_free_block / total_free_bytes)。
    /// 0.0 = 无碎片（一个连续大块）；接近 1.0 = 高度碎片化。
    pub external_fragmentation: f64,
}

/// 从 free region 列表计算碎片指标。
pub fn compute_metrics(free_regions: &[(usize, usize)]) -> FragmentationMetrics {
    let free_block_count = free_regions.len();
    let total_free_bytes: usize = free_regions.iter().map(|(_, s)| s).sum();
    let largest_free_block = free_regions.iter().map(|(_, s)| *s).max().unwrap_or(0);

    let external_fragmentation = if total_free_bytes > 0 {
        1.0 - (largest_free_block as f64 / total_free_bytes as f64)
    } else {
        0.0
    };

    FragmentationMetrics {
        free_block_count,
        total_free_bytes,
        largest_free_block,
        external_fragmentation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_reclaim_finds_contiguous_trailing_free() {
        // 空闲区 [9000, 10000) 接到 heap_ptr=10000
        // 应回退 heap_ptr 到 9000
        let regions = vec![(1000, 500), (9000, 1000)];
        let result = reclaim_trailing_free_space_impl(&regions, 10000, 1000);
        assert_eq!(result.new_heap_ptr, 9000);
        assert_eq!(result.reclaimed_bytes, 1000);
    }

    #[test]
    fn tail_reclaim_chains_multiple_adjacent() {
        // [8000,1000) + [9000,1000) 接到 heap_ptr=10000
        // 应回退到 8000
        let regions = vec![(8000, 1000), (9000, 1000)];
        let result = reclaim_trailing_free_space_impl(&regions, 10000, 1000);
        assert_eq!(result.new_heap_ptr, 8000);
        assert_eq!(result.reclaimed_bytes, 2000);
    }

    #[test]
    fn tail_reclaim_no_trailing_free() {
        // [1000,500) 不接到 heap_ptr=10000 → 不回退
        let regions = vec![(1000, 500)];
        let result = reclaim_trailing_free_space_impl(&regions, 10000, 1000);
        assert_eq!(result.new_heap_ptr, 10000);
        assert_eq!(result.reclaimed_bytes, 0);
    }

    #[test]
    fn tail_reclaim_gap_prevents_reclaim() {
        // [8000,500) + [9000,500) 但 8500-9000 有活对象 → 只回收 [9000,500)
        let regions = vec![(8000, 500), (9000, 500)];
        let result = reclaim_trailing_free_space_impl(&regions, 9500, 1000);
        assert_eq!(result.new_heap_ptr, 9000);
        assert_eq!(result.reclaimed_bytes, 500);
    }

    #[test]
    fn tail_reclaim_empty_regions() {
        let result = reclaim_trailing_free_space_impl(&[], 10000, 1000);
        assert_eq!(result.new_heap_ptr, 10000);
        assert_eq!(result.reclaimed_bytes, 0);
    }

    #[test]
    fn tail_reclaim_heap_ptr_at_start() {
        let result = reclaim_trailing_free_space_impl(&[(1000, 500)], 1000, 1000);
        assert_eq!(result.new_heap_ptr, 1000);
        assert_eq!(result.reclaimed_bytes, 0);
    }

    #[test]
    fn metrics_zero_free() {
        let m = compute_metrics(&[]);
        assert_eq!(m.free_block_count, 0);
        assert_eq!(m.total_free_bytes, 0);
        assert_eq!(m.largest_free_block, 0);
        assert_eq!(m.external_fragmentation, 0.0);
    }

    #[test]
    fn metrics_single_block_no_fragmentation() {
        let m = compute_metrics(&[(1000, 500)]);
        assert_eq!(m.free_block_count, 1);
        assert_eq!(m.total_free_bytes, 500);
        assert_eq!(m.largest_free_block, 500);
        assert_eq!(m.external_fragmentation, 0.0);
    }

    #[test]
    fn metrics_high_fragmentation() {
        // 10 个 50 字节的小块 vs 总 500 字节
        let regions: Vec<(usize, usize)> = (0..10).map(|i| (i * 50, 50)).collect();
        let m = compute_metrics(&regions);
        assert_eq!(m.free_block_count, 10);
        assert_eq!(m.total_free_bytes, 500);
        assert_eq!(m.largest_free_block, 50);
        assert!((m.external_fragmentation - 0.9).abs() < 0.001);
    }

    /// 纯函数测试辅助：不依赖 GcContext，直接传参。
    fn reclaim_trailing_free_space_impl(
        free_regions: &[(usize, usize)],
        heap_ptr: usize,
        heap_start: usize,
    ) -> TailReclaimResult {
        if heap_ptr <= heap_start || free_regions.is_empty() {
            return TailReclaimResult {
                new_heap_ptr: heap_ptr,
                reclaimed_bytes: 0,
            };
        }

        let mut tail_start = heap_ptr;
        let mut sorted: Vec<(usize, usize)> = free_regions.to_vec();
        sorted.sort_by_key(|(ptr, _)| *ptr);

        loop {
            let mut found = false;
            for &(ptr, size) in sorted.iter().rev() {
                let end = ptr + size;
                if end == tail_start && ptr >= heap_start {
                    tail_start = ptr;
                    found = true;
                    break;
                }
            }
            if !found {
                break;
            }
        }

        if tail_start >= heap_ptr {
            return TailReclaimResult {
                new_heap_ptr: heap_ptr,
                reclaimed_bytes: 0,
            };
        }

        let reclaimed = heap_ptr - tail_start;
        TailReclaimResult {
            new_heap_ptr: tail_start,
            reclaimed_bytes: reclaimed,
        }
    }
}
