//! GC 宿主能力。
//!
//! 后端无关的垃圾回收抽象。本 crate 不规定 GC 算法（mark-sweep / G1 / ZGC），
//! 只定义"触发一轮回收"与"回收结果"的后端无关契约。
//!
//! # 与 runtime 内部 `runtime_gc::api::GcStats` 的关系
//!
//! `runtime_gc::api::GcStats` 是 GC 算法内部的完整可观测性结构（碎片、暂停、
//! 迁移等指标），与具体堆布局耦合。本 crate 的 [`GcOutcome`] 是**有意简化**的
//! 后端无关投影，只保留跨后端可比的指标；后端在实现时把内部统计投影为 `GcOutcome`。

use crate::heap_context::HeapContext;

/// 一轮 GC 的后端无关结果投影。
#[derive(Debug, Clone, Copy, Default)]
pub struct GcOutcome {
    /// 累计完成的 GC 轮数（含本轮）。
    pub cycle_count: u64,
    /// 本轮回收的字节数。
    pub bytes_collected: usize,
    /// 本轮耗时（微秒）。
    pub duration_us: u64,
}

/// 垃圾回收能力。方法接收后端上下文 `ctx`。
pub trait GcHost {
    /// 触发一轮 GC，返回后端无关的回收结果。
    fn gc_collect(&mut self, ctx: &mut dyn HeapContext) -> GcOutcome {
        ctx.gc_collect()
    }

    /// 当前堆已用字节数（可选实现；默认经 `ctx` 上报）。
    fn heap_used_bytes(&mut self, ctx: &mut dyn HeapContext) -> usize {
        ctx.heap_used_bytes()
    }
}
