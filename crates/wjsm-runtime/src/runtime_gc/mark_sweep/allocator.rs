//! Segregated free list（spec §9）。
//!
//! 分离适配（segregated-fit）分配器：size class table + 每 class 一个 free list。
//! 分配 O(class 数)；sweep 时按 ptr sort 线性合并后 add_free_region。
//!
//! size class table 冻结初始值（spec §9.1，P0 验证覆盖率）。
use std::collections::VecDeque;

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

/// 分离适配 free list。每 class 一个 VecDeque<(ptr, size)>，外加 big_list。
#[derive(Debug, Clone, Default)]
pub struct SegregatedFreeList {
    /// index = size_class。每元素是该 class 的空闲块队列。
    lists: Vec<VecDeque<(usize, usize)>>,
    /// 大块（> 16384）队列。
    big_list: VecDeque<(usize, usize)>,
}

impl SegregatedFreeList {
    pub fn new() -> Self {
        Self {
            lists: vec![VecDeque::new(); SIZE_CLASSES.len()],
            big_list: VecDeque::new(),
        }
    }

    /// 清空所有 class + big list（sweep 入口调用）。
    pub fn clear(&mut self) {
        for q in &mut self.lists {
            q.clear();
        }
        self.big_list.clear();
    }

    /// 接收空闲区，按 size class 入表（spec §9.4）。
    /// 不在此处做邻接合并（sweep 已线性合并过）。
    pub fn add_free_region(&mut self, ptr: usize, size: usize) {
        if size < MIN_BLOCK {
            // 太小不入表（碎片，等同泄漏直到下次 sweep）；保守起见仍记录到最小 class
            // 实际：size < 16 无法装 header，直接丢弃（sweep 不应产生这种块）
            return;
        }
        let cls = size_class(size);
        if cls == BIG_CLASS {
            self.big_list.push_back((ptr, size));
        } else {
            self.lists[cls].push_back((ptr, size));
        }
    }

    /// best-fit in class：精确 class 或更大的 class 取第一个可用块，可分割（spec §9.3）。
    /// 返回分配到的 ptr，剩余部分经 add_free_region 回灌到对应 class。
    pub fn alloc(&mut self, size: usize) -> Option<usize> {
        if size == 0 {
            return None;
        }
        let cls = size_class(size);
        // 从 cls..BIG_CLASS 找第一个非空 class（精确或更大）
        if cls < BIG_CLASS {
            for c in cls..BIG_CLASS {
                if let Some((ptr, block_size)) = self.lists[c].pop_front() {
                    self.maybe_split(ptr, size, block_size);
                    return Some(ptr);
                }
            }
        }
        // big list：找第一个 >= size 的块（first-fit；big 块稀疏，first-fit 足够）
        // 从前向后找，取下标
        let mut chosen: Option<usize> = None;
        for (i, &(_, bs)) in self.big_list.iter().enumerate() {
            if bs >= size {
                chosen = Some(i);
                break;
            }
        }
        if let Some(i) = chosen {
            let (ptr, block_size) = self.big_list.remove(i).expect("just found");
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
        self.lists.iter().map(|q| q.len()).sum::<usize>() + self.big_list.len()
    }

    /// debug：总空闲字节数。
    #[allow(dead_code)]
    pub fn total_free_bytes(&self) -> usize {
        self.lists
            .iter()
            .flat_map(|q| q.iter())
            .map(|&(_, s)| s)
            .sum::<usize>()
            + self.big_list.iter().map(|&(_, s)| s).sum::<usize>()
    }
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
        // 剩余 128 进 class 112（向上取：128 >= 112 class）
        // 128 的 class = SIZE_CLASSES 中第一个 >= 128 → 144。验证可再 alloc 112
        let p2 = fl.alloc(112);
        assert!(p2.is_some(), "剩余块应可再分配");
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
}
