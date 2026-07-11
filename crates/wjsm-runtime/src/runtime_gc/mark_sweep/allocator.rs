//! Segregated free list（spec §9）。
//!
//! 分离适配（segregated-fit）分配器：size class table + 每 class 一个按实际 size
//! 分桶的 `BTreeMap` free list。分配在 class 内 O(log n) best-fit；
//! sweep 在写入前按 ptr 邻接合并，再 rebuild_from_coalesced_regions。
//!
//! size class table 冻结初始值（spec §9.1，P0 验证覆盖率）。
use std::collections::{BTreeMap, VecDeque};

/// 冻结的 size class table（spec §9.1）。
/// 依据：对象 = 16 + cap*32（cap 4..16 → 144..528B）；数组 = 16 + len*8（len 0..128 → 16..1040B）。
pub const SIZE_CLASSES: &[usize] = &[
    16, 48, 80, 112, 144, 176, 208, 272, 336, 432, 528, 640, 768, 1024, 1536, 2048, 4096, 8192,
    16384,
];

/// > 16384 直接进 big list。
pub const BIG_CLASS: usize = SIZE_CLASSES.len();
/// 可分割的最小块（< HEADER_SIZE 无意义）。
pub const MIN_BLOCK: usize = 16;

/// 二分查找第一个 >= size 的 class index；无则 BIG_CLASS。
pub fn size_class(size: usize) -> usize {
    // 线性/二分均可；class 数 19，二分清晰
    SIZE_CLASSES
        .binary_search(&size)
        .unwrap_or_else(|i| i.min(BIG_CLASS))
}

/// 按实际 size 分桶的 free 表：key = 块真实字节数，value = 该 size 的 ptr 队列。
type SizeBuckets = BTreeMap<usize, VecDeque<usize>>;

/// 分离适配 free list。
///
/// 每 size class 一张 `BTreeMap`（实际 size → ptr 队列），外加 big list 同结构。
/// 分配用 `range(requested..)` 取最小可用桶（best-fit），避免桶内线性扫描。
#[derive(Debug, Clone, Default)]
pub struct SegregatedFreeList {
    /// index = size_class。每 class 内按实际 size 分桶。
    lists: Vec<SizeBuckets>,
    /// 大块（> 16384）按实际 size 分桶。
    big_list: SizeBuckets,
}

impl SegregatedFreeList {
    pub fn new() -> Self {
        Self {
            lists: vec![BTreeMap::new(); SIZE_CLASSES.len()],
            big_list: BTreeMap::new(),
        }
    }

    /// 清空所有 class + big list（sweep 入口调用）。
    pub fn clear(&mut self) {
        for map in &mut self.lists {
            map.clear();
        }
        self.big_list.clear();
    }

    /// 接收空闲区，按 size class 入表（spec §9.4）。
    /// 邻接合并由 sweep 在写入前完成（#116）；此处仅入表。
    pub fn add_free_region(&mut self, ptr: usize, size: usize) {
        if size < MIN_BLOCK {
            // 太小不入表（碎片，等同泄漏直到下次 sweep）；
            // 实际：size < 16 无法装 header，直接丢弃（sweep 不应产生这种块）
            return;
        }
        let cls = size_class(size);
        if cls == BIG_CLASS {
            push_bucket(&mut self.big_list, size, ptr);
        } else {
            push_bucket(&mut self.lists[cls], size, ptr);
        }
    }

    /// sweep 结束：用已按 ptr 邻接合并的区间重建整张 free list（#116）。
    pub fn rebuild_from_coalesced_regions(&mut self, regions: &[(usize, usize)]) {
        self.clear();
        for &(ptr, size) in regions {
            self.add_free_region(ptr, size);
        }
    }

    /// best-fit：从请求 size 对应 class 起向上找，class 内 `range(size..)` 取最小可用桶，
    /// 可分割（spec §9.3）。返回分配到的 ptr，剩余部分经 add_free_region 回灌。
    pub fn alloc(&mut self, size: usize) -> Option<usize> {
        if size == 0 {
            return None;
        }
        let cls = size_class(size);
        // 从 cls..BIG_CLASS 找第一个有足够大块的 class；class 内 O(log n) best-fit。
        if cls < BIG_CLASS {
            for c in cls..BIG_CLASS {
                if let Some((ptr, block_size)) = take_best_fit(&mut self.lists[c], size) {
                    self.maybe_split(ptr, size, block_size);
                    return Some(ptr);
                }
            }
        }
        // big list：同样按实际 size 做 best-fit
        if let Some((ptr, block_size)) = take_best_fit(&mut self.big_list, size) {
            self.maybe_split(ptr, size, block_size);
            Some(ptr)
        } else {
            None
        }
    }

    /// 若 block_size > size 且剩余 >= MIN_BLOCK，分割剩余回灌。
    fn maybe_split(&mut self, ptr: usize, size: usize, block_size: usize) {
        if block_size > size {
            let remaining = block_size - size;
            if remaining >= MIN_BLOCK {
                self.add_free_region(ptr + size, remaining);
            }
            // remaining < MIN_BLOCK：内碎片，不可分割，整体给本次分配（不回灌）
        }
    }

    /// debug：总空闲块数。
    #[allow(dead_code)]
    pub fn total_free_regions(&self) -> usize {
        self.lists
            .iter()
            .map(bucket_count)
            .sum::<usize>()
            + bucket_count(&self.big_list)
    }

    /// debug：总空闲字节数。
    #[allow(dead_code)]
    pub fn total_free_bytes(&self) -> usize {
        self.lists
            .iter()
            .map(bucket_bytes)
            .sum::<usize>()
            + bucket_bytes(&self.big_list)
    }
}

/// 将 ptr 压入 size 桶；桶不存在则新建。
fn push_bucket(map: &mut SizeBuckets, size: usize, ptr: usize) {
    map.entry(size).or_default().push_back(ptr);
}

/// best-fit：`range(requested..)` 取最小可用 size 桶，pop 一块；空桶删除。
/// 返回 `(ptr, block_size)`。
fn take_best_fit(map: &mut SizeBuckets, requested: usize) -> Option<(usize, usize)> {
    let block_size = *map.range(requested..).next()?.0;
    let queue = map.get_mut(&block_size)?;
    let ptr = queue.pop_front()?;
    if queue.is_empty() {
        map.remove(&block_size);
    }
    Some((ptr, block_size))
}

fn bucket_count(map: &SizeBuckets) -> usize {
    map.values().map(VecDeque::len).sum()
}

fn bucket_bytes(map: &SizeBuckets) -> usize {
    map.iter()
        .map(|(&size, q)| size.saturating_mul(q.len()))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_from_free_list_exact() {
        let mut fl = SegregatedFreeList::new();
        fl.add_free_region(1000, 144); // class 144
        assert_eq!(fl.alloc(144), Some(1000)); // 精确匹配
        assert_eq!(fl.alloc(144), None); // 已用完
    }

    #[test]
    fn alloc_splits_oversized_block() {
        let mut fl = SegregatedFreeList::new();
        fl.add_free_region(2000, 272); // class 272
        let p = fl.alloc(144).unwrap(); // 从 class 272 取，分割
        assert_eq!(p, 2000);
        // 剩余 128 进 class 144（128 向上取到 144）。验证可再 alloc 112
        let p2 = fl.alloc(112);
        assert!(p2.is_some(), "剩余块应可再分配");
        assert_eq!(p2, Some(2000 + 144));
    }

    #[test]
    fn alloc_skips_too_small_blocks_in_same_size_class() {
        let mut fl = SegregatedFreeList::new();
        // 2680 与 3856 都落在 4096 桶；BTreeMap.range 必须跳过过小键。
        fl.add_free_region(246328, 2680);

        assert_eq!(fl.alloc(3856), None);
        assert_eq!(fl.alloc(2680), Some(246328));
    }

    #[test]
    fn alloc_falls_back_to_higher_class() {
        let mut fl = SegregatedFreeList::new();
        fl.add_free_region(3000, 528); // class 528
        assert_eq!(fl.alloc(144), Some(3000)); // class 144 空，向上取 528
    }

    #[test]
    fn alloc_big_list() {
        let mut fl = SegregatedFreeList::new();
        fl.add_free_region(5000, 20000); // > 16384 → big list
        assert_eq!(fl.alloc(20000), Some(5000));
    }

    #[test]
    fn add_free_region_too_small_dropped() {
        let mut fl = SegregatedFreeList::new();
        fl.add_free_region(6000, 8); // < MIN_BLOCK(16)
        assert_eq!(fl.total_free_regions(), 0);
    }

    #[test]
    fn alloc_none_when_empty() {
        let mut fl = SegregatedFreeList::new();
        assert_eq!(fl.alloc(100), None);
    }

    #[test]
    fn size_class_lookup() {
        assert_eq!(size_class(16), 0); // 精确匹配
        assert_eq!(size_class(17), 1); // 向上取到 48
        assert_eq!(size_class(144), 4); // 精确匹配（index 4）
        assert_eq!(size_class(145), 5); // 向上取到 176
        assert_eq!(size_class(16385), BIG_CLASS);
        assert_eq!(size_class(20000), BIG_CLASS);
    }

    /// 大量过小块后仍能 O(log n) 命中更大块，且不误取过小桶。
    #[test]
    fn alloc_best_fit_skips_many_undersized_then_hits_larger() {
        let mut fl = SegregatedFreeList::new();
        // 同属 4096 class：先塞 50 个 2000 字节块，再塞一个 3856。
        for i in 0..50 {
            fl.add_free_region(10_000 + i * 4096, 2000);
        }
        fl.add_free_region(999_000, 3856);

        assert_eq!(fl.alloc(3000), Some(999_000)); // best-fit 直取 3856，跳过 2000
        // 剩余 3856-3000=856 → class 1024；2000 块仍在
        assert_eq!(fl.total_free_regions(), 51);
        assert_eq!(fl.alloc(2000), Some(10_000));
    }

    /// best-fit：同 class 内有 80/112/144 时，alloc(80) 取 80 而非更大块。
    #[test]
    fn alloc_best_fit_prefers_smallest_sufficient_block() {
        let mut fl = SegregatedFreeList::new();
        // 80/112/144 分属不同 class；从 80 class 起应精确命中 80。
        fl.add_free_region(3000, 144);
        fl.add_free_region(2000, 112);
        fl.add_free_region(1000, 80);

        assert_eq!(fl.alloc(80), Some(1000));
        // 112/144 仍在
        assert_eq!(fl.total_free_regions(), 2);
        assert_eq!(fl.alloc(112), Some(2000));
        assert_eq!(fl.alloc(144), Some(3000));
    }

    /// free 后可 reuse；split 回灌的 remainder 可再次分配。
    #[test]
    fn free_reuse_and_split_remainder_reinsert() {
        let mut fl = SegregatedFreeList::new();
        fl.add_free_region(4000, 528);
        let p = fl.alloc(144).unwrap();
        assert_eq!(p, 4000);
        // 528-144=384 → 进 class 432
        assert_eq!(fl.total_free_bytes(), 384);
        assert_eq!(fl.alloc(336), Some(4000 + 144));
        // 384-336=48 回灌
        assert_eq!(fl.alloc(48), Some(4000 + 144 + 336));
        assert_eq!(fl.total_free_regions(), 0);

        // free 回灌后可再次 reuse
        fl.add_free_region(4000, 144);
        assert_eq!(fl.alloc(144), Some(4000));
    }

    /// rebuild 已合并区间：空闲字节/块数与输入一致，可整块取出。
    #[test]
    fn rebuild_from_coalesced_preserves_bytes_and_regions() {
        let regions = [(1000, 224), (2000, 80), (3000, 20000)];
        let mut fl = SegregatedFreeList::new();
        fl.rebuild_from_coalesced_regions(&regions);

        assert_eq!(fl.total_free_regions(), 3);
        assert_eq!(fl.total_free_bytes(), 224 + 80 + 20000);

        assert_eq!(fl.alloc(224), Some(1000));
        assert_eq!(fl.alloc(80), Some(2000));
        assert_eq!(fl.alloc(20000), Some(3000));
        assert_eq!(fl.total_free_regions(), 0);
        assert_eq!(fl.total_free_bytes(), 0);
    }

    /// big list 也走 best-fit：优先取刚好够用的最小大块。
    #[test]
    fn big_list_best_fit() {
        let mut fl = SegregatedFreeList::new();
        fl.add_free_region(10_000, 50_000);
        fl.add_free_region(20_000, 20_000);
        fl.add_free_region(30_000, 30_000);

        // 请求 25000 → 应取 30000 而非 50000
        assert_eq!(fl.alloc(25_000), Some(30_000));
        // 剩余 5000 回灌到 class 8192（非 big）
        assert_eq!(fl.alloc(20_000), Some(20_000));
        assert_eq!(fl.alloc(50_000), Some(10_000));
    }
}
