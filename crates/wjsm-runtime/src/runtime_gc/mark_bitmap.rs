#![allow(dead_code)]
//! Handle 标记位图（1 bit per handle）。
//!
//! Mark phase 用：标记从 roots 可达的 handle。Sweep 据此判定死块。
//! 移植自 RuntimeState.gc_mark_bits（Arc<Mutex<Vec<u64>>>），封装为独立类型便于算法内部持有。

/// 1 bit per handle 的位图。用 `Vec<u64>` 存储（每 word 64 bit）。
#[derive(Debug, Clone, Default)]
pub struct MarkBitmap {
    bits: Vec<u64>,
}

impl MarkBitmap {
    pub fn new() -> Self {
        Self { bits: vec![] }
    }

    /// 重置到容纳 `count` 个 handle 的容量，全部清零。
    /// 若当前容量足够，原地清零（不缩容，下标稳定 IMPL-10）。
    pub fn reset(&mut self, count: usize) {
        let words = count.div_ceil(64);
        // resize 确保 len == words（多则截断、少则补零），fill(0) 清零全部 word
        // （含扩容前残留的旧标记位，修复 #105：旧路径 resize 只补零新 word，不清旧 word）
        self.bits.resize(words, 0);
        self.bits.fill(0);
    }

    /// 标记 handle h。若 h 超出当前容量，自动扩容。
    #[allow(dead_code)]
    pub fn mark(&mut self, h: u32) {
        let (w, b) = (h as usize / 64, h as usize % 64);
        if w >= self.bits.len() {
            self.bits.resize(w + 1, 0);
        }
        self.bits[w] |= 1u64 << b;
    }

    /// 标记并返回是否首次标记（true=之前未标记，false=已标记，防重入 worklist）。
    pub fn mark_if_new(&mut self, h: u32) -> bool {
        let (w, b) = (h as usize / 64, h as usize % 64);
        if w >= self.bits.len() {
            self.bits.resize(w + 1, 0);
        }
        let mask = 1u64 << b;
        if self.bits[w] & mask != 0 {
            false
        } else {
            self.bits[w] |= mask;
            true
        }
    }

    pub fn is_marked(&self, h: u32) -> bool {
        let (w, b) = (h as usize / 64, h as usize % 64);
        w < self.bits.len() && (self.bits[w] & (1u64 << b)) != 0
    }

    /// 已标记 handle 总数（popcount）。
    pub fn popcount(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_and_is_marked() {
        let mut bm = MarkBitmap::new();
        bm.reset(10);
        assert!(!bm.is_marked(3));
        bm.mark(3);
        assert!(bm.is_marked(3));
        assert!(!bm.is_marked(4));
    }

    #[test]
    fn mark_if_new_dedup() {
        let mut bm = MarkBitmap::new();
        bm.reset(10);
        assert!(bm.mark_if_new(5)); // 首次
        assert!(!bm.mark_if_new(5)); // 重入
        assert!(bm.is_marked(5));
    }

    #[test]
    fn mark_beyond_capacity_auto_grows() {
        let mut bm = MarkBitmap::new();
        bm.reset(10);
        bm.mark(100); // 超出初始容量
        assert!(bm.is_marked(100));
    }

    #[test]
    fn popcount_counts_marked() {
        let mut bm = MarkBitmap::new();
        bm.reset(100);
        bm.mark(1);
        bm.mark(64);
        bm.mark(65);
        assert_eq!(bm.popcount(), 3);
    }

    #[test]
    fn reset_clears_all() {
        let mut bm = MarkBitmap::new();
        bm.reset(100);
        bm.mark(1);
        bm.mark(50);
        bm.reset(100);
        assert_eq!(bm.popcount(), 0);
    }

    #[test]
    fn reset_after_growth_clears_old_bits() {
        let mut bm = MarkBitmap::new();
        bm.reset(100); // words = 2
        bm.mark(1);
        bm.mark(63);
        assert_eq!(bm.popcount(), 2);
        bm.reset(200); // words = 4 (grew) — old words must clear
        assert_eq!(bm.popcount(), 0);
        // 确认扩容后新容量可用
        bm.mark(150);
        assert_eq!(bm.popcount(), 1);
    }
}
