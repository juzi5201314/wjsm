//! GC 运行时契约：根扫描、上下文与统计。
//!
//! V1 `GcAlgorithm` dyn trait 与 memory32 bump collector 已退役。
//! active collect 由 `active_v2` / `active_zgc` 按 `GcAlgorithmKind` 分派。
//!
//! **关键不变量**（v2 spec §22）：
//! - INV-C1：JS 值层引用是 handle；`obj_table[h]` 是唯一 ptr truth。
//! - INV-C2：raw ptr 不跨潜在 moving/collect GC 点；跨越必须重新 resolve。
//! - IMPL-8：`GcContext` 不持 `&mut [u8]`；每阶段重借，grow 经 `ctx.grow()`。
use crate::RuntimeState;
use crate::wasm_env::WasmEnv;
use wasmtime::{AsContextMut, StoreContextMut};

// ── 基础别名 ──
/// 对象 handle（obj_table 下标）。NaN-boxed 值的低 32 位。
pub type Handle = u32;
/// NaN-boxed JS 值（i64）。
pub type Value = i64;

/// 增量 GC 步进预算（协议模块 / 单元测试用）。
#[derive(Debug, Clone, Copy)]
pub struct StepBudget {
    pub work_bytes: usize,
    pub deadline: std::time::Instant,
}

// ── Root 发现（回调式，#6，避免每次 GC clone 整个 shadow stack）──
pub trait RootProvider {
    /// 扫描 shadow stack，对每个 root handle 调 visit。
    fn for_each_shadow_stack_root(&mut self, ctx: &mut GcContext, visit: &mut dyn FnMut(Handle));
    /// 扫描 host 侧表（promise/microtask/continuation/streams/...），含 fixed-point 驱动。
    /// `is_marked` 用于只扫描已可达 owner 的内部引用，避免侧表把 owner 反向保活。
    fn for_each_host_table_root(
        &mut self,
        ctx: &mut GcContext,
        is_marked: &mut dyn FnMut(Handle) -> bool,
        visit: &mut dyn FnMut(Handle),
    );
    /// 预留：未来精确栈扫描（WASM GC proposal / stack maps）。默认空。
    fn for_each_wasm_local_root(&mut self, _ctx: &mut GcContext, _visit: &mut dyn FnMut(Handle)) {}
}

// ── 算法运行时上下文（注入给 trait 方法） ──
//
// 【IMPL-8 / #9 关键约束】不持有 `&mut [u8]`。原因：memory.grow 可能在分配路径触发。Wasmtime 下 `memory.grow(&mut store, _)` 与
// `memory.data_mut(&store)` 都可变借用 store —— 持有 slice 时无法 grow，强行 unsafe 是 UB
// （grow 会 remap 后端 buffer，slice 悬垂）。
// 故 GcContext 持 `StoreContextMut`（由 Caller 或 Store 经 as_context_mut 产生），
// 每阶段重新 data()/data_mut()。WasmEnv 提供 Global 句柄，避免 get_export（Caller 专有）。
pub struct GcContext<'a> {
    /// wasmtime store 上下文（由 Caller 或 Store 经 as_context_mut 产生）。
    pub store: StoreContextMut<'a, RuntimeState>,
    /// WASM 导出句柄集（Global/Memory/Table，Copy），供 read_i32_global 替代。
    pub env: &'a WasmEnv,
    pub stats: GcStats,
}

impl<'a> GcContext<'a> {
    pub fn new<C: AsContextMut<Data = RuntimeState>>(
        ctx: &'a mut C,
        env: &'a WasmEnv,
        _algorithm_name: &'static str,
    ) -> Self {
        Self {
            store: ctx.as_context_mut(),
            env,
            stats: GcStats::default(),
        }
    }

    /// 读取独立影子栈 memory。
    pub fn with_shadow_memory<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> R {
        let data = self.env.shadow_memory.data(&self.store);
        f(data)
    }

    /// 读主 memory。借用 store，离开作用域后可再 grow / data_mut。
    pub fn with_memory<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> R {
        let data = self.env.memory.data(&self.store);
        f(data)
    }

    /// 写 memory。单独可变借用。
    pub fn with_memory_mut<R>(&mut self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        let data = self.env.memory.data_mut(&mut self.store);
        f(data)
    }

    /// 扩页。必须在外层调用，不持 slice。失败返回 Err。
    pub fn grow(&mut self, pages: u64) -> Result<u64, ()> {
        self.env.memory.grow(&mut self.store, pages).map_err(|_| ())
    }

    /// 读/写 RuntimeState（store.data_mut）。
    pub fn with_state<R>(&mut self, f: impl FnOnce(&mut RuntimeState) -> R) -> R {
        f(self.store.data_mut())
    }

    /// 当前 GC epoch。debug INV-C2 用：任何可能改写 obj_table ptr/色位的 GC 点递增。
    #[cfg(debug_assertions)]
    #[allow(dead_code)]
    pub fn gc_epoch(&self) -> u64 {
        self.store
            .data()
            .gc_epoch
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 递增 GC epoch。任何可能改变 `obj_table` 指针或色位的 GC 点完成后调用。
    pub fn increment_gc_epoch(&mut self) -> u64 {
        self.store
            .data()
            .gc_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    /// 设置 v2 分配窗口。P1 前这些 globals 不存在，因此按 Option 容错。
    pub fn alloc_window_set(&mut self, ptr: usize, end: usize) {
        if let Some(global) = self.env.alloc_ptr {
            let _ = global.set(&mut self.store, wasmtime::Val::I32(ptr as i32));
        }
        if let Some(global) = self.env.alloc_end {
            let _ = global.set(&mut self.store, wasmtime::Val::I32(end as i32));
        }
    }
}

// ── GC 统计 ──
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CycleKind {
    #[default]
    Full,
    Young,
    Mixed,
    ZgcCycle,
    Step,
}

impl CycleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Young => "young",
            Self::Mixed => "mixed",
            Self::ZgcCycle => "zgc-cycle",
            Self::Step => "step",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GcStats {
    pub marked: usize,
    pub swept: usize,
    pub freed_bytes: usize,
    pub elapsed: std::time::Duration,
    // ── 碎片治理指标（issue #332）──
    /// 空闲块总数（sweep 后）。
    pub free_block_count: usize,
    /// 总空闲字节数（sweep 后）。
    pub total_free_bytes: usize,
    /// 最大连续空闲块字节数。
    pub largest_free_block: usize,
    /// 外部碎片率：1 - (largest_free_block / total_free_bytes)。
    pub external_fragmentation: f64,
    /// 本次 sweep 尾部空间回收的字节数（heap_ptr 降低量）。
    pub tail_reclaimed_bytes: usize,
    /// 堆已用字节（heap_ptr - heap_start，sweep 后）。
    pub heap_used_bytes: usize,
    // ── v2 可观测性指标（spec §17）──
    pub cycle_kind: CycleKind,
    pub pause_ns_max: u64,
    pub pause_ns_total: u64,
    pub pause_count: usize,
    pub relocated_bytes: usize,
    pub relocated_objects: usize,
    pub committed_pages: usize,
    pub free_bytes_reusable: usize,
    pub regions_total: usize,
    pub regions_free: usize,
    pub regions_eden: usize,
    pub regions_survivor: usize,
    pub regions_old: usize,
    pub regions_humongous: usize,
    pub satb_flushes: usize,
    pub barrier_events: usize,
    pub rset_cards: usize,
    pub rset_precise_slots: usize,
    pub scan_oblets: usize,
    pub load_barrier_mark_hits: usize,
    pub load_barrier_relocate_hits: usize,
    // ── 归一化 gate 分子/分母（spec §18.4，thread CPU time）──
    /// 本轮 GC 工作 CPU 纳秒：worker + pause + mutator assist 全部线程 CPU 之和。
    pub gc_cpu_ns: u64,
    /// 本轮 young/old mark CPU 纳秒（mark worker 与 mark pause）。
    pub mark_cpu_ns: u64,
    /// 本轮 relocate worker 与 assist CPU 纳秒。
    pub relocation_cpu_ns: u64,
    /// mark-end physical live bytes：`mark CPU / live byte` 的分母。
    pub mark_live_bytes: u64,
}

impl GcStats {
    pub fn record_pause(&mut self, pause: std::time::Duration) {
        let pause_ns = nanos_u64(pause);
        self.pause_ns_max = self.pause_ns_max.max(pause_ns);
        self.pause_ns_total = self.pause_ns_total.saturating_add(pause_ns);
        self.pause_count = self.pause_count.saturating_add(1);
    }

    pub fn with_elapsed_pause(mut self) -> Self {
        if !self.elapsed.is_zero() {
            self.record_pause(self.elapsed);
        }
        self
    }

    pub fn ensure_pause_from_elapsed(&mut self) {
        if self.pause_count == 0 && !self.elapsed.is_zero() {
            self.record_pause(self.elapsed);
        }
    }

    pub fn has_pause_observation(&self) -> bool {
        self.pause_count != 0 || !self.elapsed.is_zero()
    }

    pub fn merge_from(&mut self, extra: &Self) {
        self.marked = self.marked.saturating_add(extra.marked);
        self.swept = self.swept.saturating_add(extra.swept);
        self.freed_bytes = self.freed_bytes.saturating_add(extra.freed_bytes);
        self.elapsed += extra.elapsed;
        self.free_block_count = extra.free_block_count;
        self.total_free_bytes = extra.total_free_bytes;
        self.largest_free_block = extra.largest_free_block;
        self.external_fragmentation = extra.external_fragmentation;
        self.tail_reclaimed_bytes = self
            .tail_reclaimed_bytes
            .saturating_add(extra.tail_reclaimed_bytes);
        self.heap_used_bytes = extra.heap_used_bytes;
        self.pause_ns_max = self.pause_ns_max.max(extra.pause_ns_max);
        self.pause_ns_total = self.pause_ns_total.saturating_add(extra.pause_ns_total);
        self.pause_count = self.pause_count.saturating_add(extra.pause_count);
        self.relocated_bytes = self.relocated_bytes.saturating_add(extra.relocated_bytes);
        self.relocated_objects = self
            .relocated_objects
            .saturating_add(extra.relocated_objects);
        self.committed_pages = extra.committed_pages;
        self.free_bytes_reusable = extra.free_bytes_reusable;
        self.regions_total = extra.regions_total;
        self.regions_free = extra.regions_free;
        self.regions_eden = extra.regions_eden;
        self.regions_survivor = extra.regions_survivor;
        self.regions_old = extra.regions_old;
        self.regions_humongous = extra.regions_humongous;
        self.satb_flushes = self.satb_flushes.saturating_add(extra.satb_flushes);
        self.barrier_events = self.barrier_events.saturating_add(extra.barrier_events);
        self.rset_cards = extra.rset_cards;
        self.rset_precise_slots = extra.rset_precise_slots;
        self.scan_oblets = self.scan_oblets.saturating_add(extra.scan_oblets);
        self.load_barrier_mark_hits = self
            .load_barrier_mark_hits
            .saturating_add(extra.load_barrier_mark_hits);
        self.load_barrier_relocate_hits = self
            .load_barrier_relocate_hits
            .saturating_add(extra.load_barrier_relocate_hits);
        self.gc_cpu_ns = self.gc_cpu_ns.saturating_add(extra.gc_cpu_ns);
        self.mark_cpu_ns = self.mark_cpu_ns.saturating_add(extra.mark_cpu_ns);
        self.relocation_cpu_ns = self
            .relocation_cpu_ns
            .saturating_add(extra.relocation_cpu_ns);
        self.mark_live_bytes = self.mark_live_bytes.saturating_add(extra.mark_live_bytes);
    }
}

fn nanos_u64(duration: std::time::Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::{CycleKind, GcStats};
    use std::time::Duration;

    #[test]
    fn gc_stats_record_pause_tracks_max_total_and_count() {
        let mut stats = GcStats::default();

        stats.record_pause(Duration::from_nanos(7));
        stats.record_pause(Duration::from_nanos(3));

        assert_eq!(stats.pause_ns_max, 7);
        assert_eq!(stats.pause_ns_total, 10);
        assert_eq!(stats.pause_count, 2);
    }

    #[test]
    fn gc_stats_merge_preserves_existing_and_v2_fields() {
        let mut stats = GcStats {
            cycle_kind: CycleKind::Full,
            marked: 2,
            swept: 1,
            freed_bytes: 10,
            elapsed: Duration::from_nanos(5),
            heap_used_bytes: 90,
            pause_ns_max: 5,
            pause_ns_total: 5,
            pause_count: 1,
            relocated_bytes: 8,
            relocated_objects: 1,
            barrier_events: 2,
            rset_cards: 1,
            load_barrier_mark_hits: 1,
            ..GcStats::default()
        };
        let extra = GcStats {
            cycle_kind: CycleKind::Mixed,
            marked: 3,
            swept: 4,
            freed_bytes: 20,
            elapsed: Duration::from_nanos(7),
            heap_used_bytes: 70,
            pause_ns_max: 7,
            pause_ns_total: 7,
            pause_count: 1,
            relocated_bytes: 16,
            relocated_objects: 2,
            committed_pages: 5,
            free_bytes_reusable: 4096,
            regions_total: 6,
            regions_free: 2,
            satb_flushes: 1,
            barrier_events: 3,
            rset_cards: 2,
            rset_precise_slots: 1,
            load_barrier_relocate_hits: 4,
            ..GcStats::default()
        };

        stats.merge_from(&extra);

        assert_eq!(stats.cycle_kind, CycleKind::Full);
        assert_eq!(stats.marked, 5);
        assert_eq!(stats.swept, 5);
        assert_eq!(stats.freed_bytes, 30);
        assert_eq!(stats.elapsed, Duration::from_nanos(12));
        assert_eq!(stats.heap_used_bytes, 70);
        assert_eq!(stats.pause_ns_max, 7);
        assert_eq!(stats.pause_ns_total, 12);
        assert_eq!(stats.pause_count, 2);
        assert_eq!(stats.relocated_bytes, 24);
        assert_eq!(stats.relocated_objects, 3);
        assert_eq!(stats.committed_pages, 5);
        assert_eq!(stats.free_bytes_reusable, 4096);
        assert_eq!(stats.regions_total, 6);
        assert_eq!(stats.regions_free, 2);
        assert_eq!(stats.satb_flushes, 1);
        assert_eq!(stats.barrier_events, 5);
        assert_eq!(stats.rset_cards, 2);
        assert_eq!(stats.rset_precise_slots, 1);
        assert_eq!(stats.load_barrier_mark_hits, 1);
        assert_eq!(stats.load_barrier_relocate_hits, 4);
    }
}
