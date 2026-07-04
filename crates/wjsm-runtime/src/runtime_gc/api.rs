//! GC trait 框架（spec §6）。
//!
//! `GcAlgorithm` 是 v2 生命周期接口：算法接管动态堆域、处理 slow-path 分配、
//! safepoint 步进、完整回收，以及 G1/ZGC 所需的 barrier/load hook。
//!
//! **关键不变量**（v2 spec §22）：
//! - INV-C1：JS 值层引用是 handle；`obj_table[h]` 是唯一 ptr truth。
//! - INV-C2：raw ptr 不跨潜在 moving/collect GC 点；跨越必须重新 resolve。
//! - IMPL-8：`GcContext` 不持 `&mut [u8]`；每阶段重借，grow 经 `ctx.grow()`。
#![allow(dead_code)]
use crate::RuntimeState;
use crate::wasm_env::WasmEnv;
use wasmtime::{AsContextMut, StoreContextMut};

// ── 基础别名 ──
/// 对象 handle（obj_table 下标）。NaN-boxed 值的低 32 位。
pub type Handle = u32;
/// NaN-boxed JS 值（i64）。
pub type Value = i64;

/// fast-path 分配窗口耗尽后交给算法的完整 slow-path 请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocRequest {
    pub size: usize,
    pub heap_type: u8,
    pub capacity: u32,
}

/// 增量 GC 步进预算，由调度器按 pause target 与吞吐估算生成。
#[derive(Debug, Clone, Copy)]
pub struct StepBudget {
    pub work_bytes: usize,
    pub deadline: std::time::Instant,
}

/// 单次 safepoint 步进结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    Idle,
    Progress { remaining_estimate: usize },
    CycleComplete,
}

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

/// v2 算法接口：用完整生命周期方法取代 v1 `Allocator + Marker + Sweeper` 切片。
/// 默认 hook 只用于非对应算法的不可达路径，不提供行为兜底。
pub trait GcAlgorithm: Send + Sync {
    fn name(&self) -> &'static str;

    /// 实例化后一次性接管动态堆域 `[dynamic_start, heap_limit)`。
    fn attach_heap(&mut self, ctx: &mut GcContext, dynamic_start: usize);

    /// 分配 slow-path：返回线性内存 ptr，handle 注册仍由调用方完成。
    fn alloc_slow(
        &mut self,
        ctx: &mut GcContext,
        roots: &mut dyn RootProvider,
        req: AllocRequest,
    ) -> Option<usize>;

    /// safepoint 轮询入口，按预算推进增量工作。
    fn safepoint_step(
        &mut self,
        ctx: &mut GcContext,
        roots: &mut dyn RootProvider,
        budget: StepBudget,
    ) -> StepOutcome;

    /// 显式 `gc()` / OOM 兜底入口：同步跑完当前或新周期。
    fn collect_full(&mut self, ctx: &mut GcContext, roots: &mut dyn RootProvider) -> GcStats;

    /// ZGC load barrier slow-path；非 ZGC 算法不应被调用。
    fn load_barrier_slow(&mut self, ctx: &mut GcContext, h: Handle) -> u32 {
        let _ = (ctx, h);
        debug_assert!(false, "load_barrier_slow called on non-zgc algorithm");
        0
    }

    /// 统一 barrier event buffer flush。硬约束：只 drain，不 collect/grow/move/recolor。
    fn barrier_flush(&mut self, ctx: &mut GcContext) {
        let _ = ctx;
    }

    /// host 侧堆写 hook，由 `heap_access` 统一入口调用。
    fn on_host_write(
        &mut self,
        ctx: &mut GcContext,
        target: Handle,
        slot_addr: usize,
        old_val: Value,
        new_val: Value,
    ) {
        let _ = (ctx, target, slot_addr, old_val, new_val);
    }

    /// host 侧解引用 hook；ZGC relocate 期可同步 heal 后返回新 ptr。
    fn on_host_resolve(&mut self, ctx: &mut GcContext, h: Handle) -> Option<usize> {
        let _ = (ctx, h);
        None
    }

    fn last_stats(&self) -> &GcStats;
}

// ── 算法运行时上下文（注入给 trait 方法） ──
//
// 【IMPL-8 / #9 关键约束】不持有 `&mut [u8]`。原因：gc_alloc_slow 在 mark/sweep 后可能
// 仍不够空间，需 memory.grow()。Wasmtime 下 `memory.grow(&mut store, _)` 与
// `memory.data_mut(&store)` 都可变借用 store —— 持有 slice 时无法 grow，强行 unsafe 是 UB
// （grow 会 remap 后端 buffer，slice 悬垂）。
// 故 GcContext 持 `StoreContextMut`（由 Caller 或 Store 经 as_context_mut 产生），
// 每阶段重新 data()/data_mut()。WasmEnv 提供 Global 句柄，避免 get_export（Caller 专有）。
pub struct GcContext<'a> {
    /// wasmtime store 上下文（由 Caller 或 Store 经 as_context_mut 产生）。
    pub store: StoreContextMut<'a, RuntimeState>,
    /// WASM 导出句柄集（Global/Memory/Table，Copy），供 read_i32_global 替代。
    pub env: &'a WasmEnv,
    /// 仅用于日志。
    pub gc_algorithm_name: &'static str,
    pub stats: GcStats,
}

impl<'a> GcContext<'a> {
    pub fn new<C: AsContextMut<Data = RuntimeState>>(
        ctx: &'a mut C,
        env: &'a WasmEnv,
        algorithm_name: &'static str,
    ) -> Self {
        Self {
            store: ctx.as_context_mut(),
            env,
            gc_algorithm_name: algorithm_name,
            stats: GcStats::default(),
        }
    }

    /// 读 memory。借用 store，离开作用域后可再 grow / data_mut。
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
    pub fn gc_epoch(&self) -> u64 {
        self.store
            .data()
            .gc_epoch
            .load(std::sync::atomic::Ordering::Relaxed)
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
}
