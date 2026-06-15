//! GC trait 框架（spec §6）。
//!
//! 算法 trait 抽象：`GcAlgorithm: Allocator + Marker + Sweeper`。
//! 默认实现 `MarkSweepCollector`（non-moving + segregated free list）。
//! 预留 generational/incremental/parallel 接入点（WriteBarrier/ReadBarrier/
//! HeapRegionManager/mark_step/sweep_step，默认 no-op）。
//!
//! **稳定性承诺**（spec 附录 D）：trait 签名、GcContext 字段集（只增不减）、
//! Handle/Value 别名、fast-path 物理边界均稳定，后续算法只 impl 新 struct 不改框架。
//!
//! **关键不变量**（spec §18）：
//! - INV-C 对象永不动（non-moving）：所有算法实现必须维护，否则 WASM locals 失效 → O2 复现。
//! - IMPL-8 `GcContext` 不持 `&mut [u8]`；每阶段 `with_memory`/`with_memory_mut` 重借；
//!   grow 经 `ctx.grow()`（#9，grow 借用安全）。
use crate::RuntimeState;
use wasmtime::{Caller, Memory};

// ── 基础别名 ──
/// 对象 handle（obj_table 下标）。NaN-boxed 值的低 32 位。
pub type Handle = u32;
/// NaN-boxed JS 值（i64）。
pub type Value = i64;

// ── 对象元信息查询（所有算法共享，只读） ──
/// 查询对象堆元信息。算法通过 GcContext.with_memory 现场算，不缓存。
/// 本 trait 保留为能力注入接口（未来 moving 算法可注入 forwarding 查询）。
pub trait HeapObjectQuery {
    /// 从 header 算对象总大小（16B header + payload）。
    fn object_size(&self, h: Handle) -> usize;
    /// obj_table[h] → linear memory ptr。
    fn object_ptr(&self, h: Handle) -> usize;
    /// 对象 heap_type tag（HEAP_TYPE_OBJECT/ARRAY/...）。
    fn heap_type(&self, h: Handle) -> u8;
}

// ── 分配器：fast-path 固定烧进 WASM，slow-path 走 trait ──
pub trait Allocator {
    /// fast-path bump 失败后调用。策略决定：free list / GC / grow。
    /// 返回分配到的**线性内存 ptr**（仅地址，不含 handle 注册——调用方自己 take_or_alloc_handle）；
    /// None 表示真 OOM（由 trampoline trap）。
    fn alloc_slow(
        &mut self,
        ctx: &mut GcContext,
        size: usize,
        heap_type: u8,
        capacity: u32,
    ) -> Option<usize>;
    /// 接收被 sweep 释放的空闲区（MarkSweep 用）。
    fn add_free_region(&mut self, ptr: usize, size: usize);
    /// 预留：未来 TLAB / region-local 分配。默认 None。
    fn alloc_thread_local(&mut self, _ctx: &mut GcContext, _size: usize) -> Option<usize> {
        None
    }
}

// ── Root 发现（回调式，#6，避免每次 GC clone 整个 shadow stack）──
pub trait RootProvider {
    /// 扫描 shadow stack，对每个 root handle 调 visit。
    fn for_each_shadow_stack_root(&mut self, ctx: &mut GcContext, visit: &mut dyn FnMut(Handle));
    /// 扫描 host 侧表（promise/microtask/continuation/streams/...），含 fixed-point 驱动。
    fn for_each_host_table_root(&mut self, ctx: &mut GcContext, visit: &mut dyn FnMut(Handle));
    /// 预留：未来精确栈扫描（WASM GC proposal / stack maps）。默认空。
    fn for_each_wasm_local_root(&mut self, _ctx: &mut GcContext, _visit: &mut dyn FnMut(Handle)) {}
}

// ── Mark 策略 ──
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkProgress {
    /// mark 完成。
    Complete,
    /// incremental：剩余 work 估算。
    Pending(usize),
}

pub trait Marker {
    /// 一次性 mark：seed roots → drain worklist（IMPL-6 显式 worklist，不递归）。
    fn mark(&mut self, ctx: &mut GcContext, roots: &mut dyn Iterator<Item = Handle>);
    fn is_marked(&self, h: Handle) -> bool;
    /// 预留：incremental mark 步进接口。默认一次性完成（non-incremental）。
    fn mark_step(&mut self, _ctx: &mut GcContext, _budget: usize) -> MarkProgress {
        MarkProgress::Complete
    }
}

// ── Sweep 策略 ──
pub trait Sweeper {
    /// 一次性 sweep：按 ptr sort → 线性合并相邻 unmarked → add_free_region
    /// → 清空 unmarked handle 槽 → process weak refs（IMPL-7 sort 必需）。
    fn sweep(&mut self, ctx: &mut GcContext);
    /// 预留：concurrent sweep 步进。默认一次性完成。
    fn sweep_step(&mut self, ctx: &mut GcContext, _budget: usize) -> usize {
        self.sweep(ctx);
        0
    }
}

// ── 预留 hook：region / card-table / barrier（generational/G1 用） ──
/// 预留 region 管理（generational/G1）。MarkSweep 不实现此 trait。
pub trait HeapRegionManager {
    type Region;
    type Card;
    fn regions(&self) -> std::slice::Iter<'_, Self::Region>;
    fn card_for(&self, ptr: usize) -> Self::Card;
}

/// 写屏障。non-moving MarkSweep 默认 no-op（无消费者，spec §12.2 defer 到 generational）。
pub trait WriteBarrier {
    fn on_write(&mut self, _ctx: &mut GcContext, _target: Handle, _field: usize, _val: Value) {}
}

/// 读屏障。默认 no-op。
pub trait ReadBarrier {
    fn on_read(&mut self, _ctx: &mut GcContext, _target: Handle, _field: usize) {}
}

// ── 顶层算法：组装 Allocator + Marker + Sweeper ──
pub trait GcAlgorithm: Allocator + Marker + Sweeper {
    /// 完整 GC 周期：reset mark → mark roots（经 RootProvider 回调）→ fixed-point → sweep → weak refs。
    fn collect(&mut self, ctx: &mut GcContext) -> GcStats;
    fn algorithm_name(&self) -> &'static str;

    /// 带 RootProvider 的完整 collect（fixed-point host-table root 追踪，IMPL-9，spec §10）。
    /// 默认实现回退到 collect（无 fixed-point）；MarkSweepCollector 覆盖为真正的 fixed-point。
    fn collect_with_provider(
        &mut self,
        _ctx: &mut GcContext,
        _roots: &mut dyn RootProvider,
    ) -> GcStats {
        // 默认：无 RootProvider 信息，回退到 collect（不应被 P4 集成调用）。
        let empty: std::iter::Empty<Handle> = std::iter::empty();
        self.mark(_ctx, &mut Box::new(empty) as _);
        self.sweep(_ctx);
        _ctx.stats.clone()
    }
}

// ── 算法运行时上下文（注入给 trait 方法） ──
//
// 【IMPL-8 / #9 关键约束】不持有 `&mut [u8]`。原因：gc_alloc_slow 在 mark/sweep 后可能
// 仍不够空间，需 memory.grow()。Wasmtime 下 `memory.grow(&mut store, _)` 与
// `memory.data_mut(&store)` 都可变借用 store —— 持有 slice 时无法 grow，强行 unsafe 是 UB
// （grow 会 remap 后端 buffer，slice 悬垂）。
// 故 GcContext 持 `&mut Caller`，每阶段重新 data()/data_mut()。
pub struct GcContext<'a, 'b> {
    pub caller: &'a mut Caller<'b, RuntimeState>,
    /// wasmtime Memory 句柄（轻量，不含借用）。
    pub memory: Memory,
    /// 仅用于日志。
    pub gc_algorithm_name: &'static str,
    pub stats: GcStats,
}

impl<'a, 'b> GcContext<'a, 'b> {
    pub fn new(
        caller: &'a mut Caller<'b, RuntimeState>,
        memory: Memory,
        algorithm_name: &'static str,
    ) -> Self {
        Self {
            caller,
            memory,
            gc_algorithm_name: algorithm_name,
            stats: GcStats::default(),
        }
    }

    /// 读 memory。借用 caller，离开作用域后可再 grow / data_mut。
    pub fn with_memory<R>(&mut self, f: impl FnOnce(&Caller<'_, RuntimeState>, &[u8]) -> R) -> R {
        let data = self.memory.data(&*self.caller);
        f(&*self.caller, data)
    }

    /// 写 memory。单独可变借用。
    /// 注：不能同时传 &mut Caller 和 &mut [u8]（双重借用 caller）。
    /// 改为只传 data slice，caller 经 self 后续访问。
    pub fn with_memory_mut<R>(&mut self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        let data = self.memory.data_mut(&mut *self.caller);
        f(data)
    }

    /// 扩页。必须在外层调用，不持 slice。失败返回 Err。
    pub fn grow(&mut self, pages: u64) -> Result<u64, ()> {
        self.memory.grow(&mut *self.caller, pages).map_err(|_| ())
    }

    /// 读/写 RuntimeState（caller.data_mut）。
    pub fn with_state<R>(&mut self, f: impl FnOnce(&mut RuntimeState) -> R) -> R {
        f(self.caller.data_mut())
    }
}

// ── GC 统计 ──
#[derive(Debug, Clone, Default)]
pub struct GcStats {
    pub marked: usize,
    pub swept: usize,
    pub freed_bytes: usize,
    pub elapsed: std::time::Duration,
}
