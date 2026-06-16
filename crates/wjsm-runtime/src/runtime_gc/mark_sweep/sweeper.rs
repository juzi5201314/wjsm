//! Sweep phase（spec §8.2，IMPL-7 按 ptr sort 必需）。
//!
//! 核心：**不改活动对象布局**（INV-D）。用 obj_table + marked bits 按 ptr 顺序重建 free list。
//!
//! 算法：
//! 1. 收集所有已分配块信息（含已死）。依赖 INV-A（obj_table 完整索引）。
//! 2. sort_by_ptr（resize INV-B 破坏 handle→ptr 单调性，sort 必需）。
//! 3. 线性扫描合并相邻 unmarked 块 → free_list.add_free_region。
//! 4. 清空 unmarked handle 的 obj_table 槽（置 0），推入 freed_handles（IMPL-10/#7 复用）。
use crate::runtime_gc::api::GcContext;
use crate::runtime_gc::context::object_size_from_memory;
use crate::runtime_gc::mark_sweep::MarkSweepCollector;

/// 收集的块信息：(ptr, size, handle, is_marked)。
struct BlockInfo {
    ptr: usize,
    size: usize,
    handle: u32,
    marked: bool,
}

pub fn sweep(collector: &mut MarkSweepCollector, ctx: &mut GcContext) {
    let obj_table_ptr = ctx.obj_table_ptr();
    let count = ctx.obj_table_count();

    // 1. 收集所有块信息（含已死）
    let blocks: Vec<BlockInfo> = ctx.with_memory(|_caller, data| {
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
            let Some(size) = object_size_from_memory(data, ptr) else {
                continue;
            };
            let marked = collector.mark_bits.is_marked(h);
            out.push(BlockInfo {
                ptr,
                size,
                handle: h,
                marked,
            });
        }
        out
    });

    // 2. sort by ptr（IMPL-7）
    let mut blocks = blocks;
    blocks.sort_by_key(|b| b.ptr);

    // 3. 线性扫描合并相邻 unmarked 块 → add_free_region
    collector.free_list.clear();
    let mut i = 0;
    while i < blocks.len() {
        if blocks[i].marked {
            i += 1;
            continue;
        }
        // 开始一个 unmarked run
        let run_ptr = blocks[i].ptr;
        let mut run_end = blocks[i].ptr + blocks[i].size;
        i += 1;
        // 合并后续**物理相邻**（next.ptr == run_end）的 unmarked 块。
        // 由于按 ptr 升序且无重叠（INV-A），相邻 = next.ptr == run_end。
        // 若 next.ptr > run_end（中间有活对象 gap），停止合并；该 next 块由外层循环重新作为新 run。
        while i < blocks.len() && !blocks[i].marked && blocks[i].ptr == run_end {
            run_end = blocks[i].ptr + blocks[i].size;
            i += 1;
        }
        collector
            .free_list
            .add_free_region(run_ptr, run_end - run_ptr);
    }

    // 4. 清空 unmarked handle 槽 + 推入 freed_handles
    let mut freed = Vec::new();
    ctx.with_memory_mut(|data| {
        for b in &blocks {
            if !b.marked {
                let addr = obj_table_ptr + b.handle as usize * 4;
                if addr + 4 <= data.len() {
                    data[addr..addr + 4].copy_from_slice(&0u32.to_le_bytes());
                }
                freed.push(b.handle);
            }
        }
    });

    // 5. 回收 resize-abandoned 区域（P4-blocker #1，INV-B）。
    //    grow_array/grow_object 重写 obj_table 槽到新 ptr 后，旧 ptr 区域不再被
    //    obj_table 索引，步骤 1-3 的块扫描看不到 → 单独注册到 abandoned_regions。
    //    这里把它们并入 free list（按 ptr 进表，alloc_slow best-fit 合并），然后清空。
    let abandoned: Vec<(usize, usize)> = ctx.with_state(|st| {
        st.abandoned_regions_for_gc()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default()
    });
    for (ptr, size) in abandoned {
        collector.free_list.add_free_region(ptr, size);
    }

    ctx.stats.swept = freed.len();
    let freed_bytes: usize = blocks.iter().filter(|b| !b.marked).map(|b| b.size).sum();
    ctx.stats.freed_bytes = freed_bytes;
    collector.freed_handles.extend(freed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_gc::mark_sweep::MarkSweepCollector;

    /// P4-blocker #1：验证 resize-abandoned 区域经 sweeper 步骤 5 路径进入 free list。
    /// grow_array/grow_object 重写 obj_table 槽后旧 ptr 不可达；这里单独验证
    /// "abandoned (ptr, size) → free_list.add_free_region → alloc 可复用" 的回收链路。
    #[test]
    fn abandoned_region_recovers_into_free_list() {
        let mut collector = MarkSweepCollector::new();
        // 模拟一次 array resize：旧区域 ptr=2000, size=80（16 + 8*8），新区域在更高地址。
        // 块扫描（步骤 1-3）只看到新区域，旧区域经 abandoned 注入步骤 5。
        collector.free_list.add_free_region(2000, 80);
        // 该区域应可被 alloc 复用（best-fit：80 进 class 80，alloc(80) 精确命中）。
        assert_eq!(collector.free_list.alloc(80), Some(2000));
        // 用尽后应 None（验证不是幽灵复用）。
        assert_eq!(collector.free_list.alloc(80), None);
    }

    /// abandoned 区域与 sweep 释放的 unmarked 块独立入表，互不干扰。
    #[test]
    fn abandoned_coexists_with_swept_blocks() {
        let mut collector = MarkSweepCollector::new();
        // sweep 步骤 3 释放一块
        collector.free_list.add_free_region(1000, 144);
        // abandoned 注入另一块（不同 ptr）
        collector.free_list.add_free_region(5000, 272);
        assert_eq!(collector.free_list.alloc(144), Some(1000));
        assert_eq!(collector.free_list.alloc(272), Some(5000));
    }
}
