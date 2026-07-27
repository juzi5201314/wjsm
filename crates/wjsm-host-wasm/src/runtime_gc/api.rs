//! GC 运行时契约：根扫描、上下文与统计。
//!
//! V1 `GcAlgorithm` dyn trait 与 memory32 bump collector 已退役。
//! active collect 由 `active_v2` / `active_zgc` 按 `GcAlgorithmKind` 分派。
//!
//! **关键不变量**（v2 spec §22）：
//! - INV-C1：JS 值层引用是 handle；`obj_table[h]` 是唯一 ptr truth。
//! - INV-C2：raw ptr 不跨潜在 moving/collect GC 点；跨越必须重新 resolve。
//! - IMPL-8：`GcContext` 不持 `&mut [u8]`；每阶段重借，grow 经 `ctx.grow()`。
//!
//! 后端无关统计类型（`CycleKind`/`GcStats`/`StepBudget` 等）来自 `wjsm-gc`；
//! 本文件只保留绑 wasmtime 的 `GcContext` / `RootProvider`。

use crate::RuntimeState;
use crate::wasm_env::WasmEnv;
use wasmtime::{AsContextMut, StoreContextMut};

// ── 从 wjsm-gc re-export 的后端无关契约 ──
#[allow(unused_imports)]
pub use wjsm_gc::api::{
    CycleKind, GcExecutionStats, GcStats, Handle, MemoryFootprintSample, StepBudget, Value,
};

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
