//! 可插拔 GC 框架（spec §6）。
//!
//! 算法与堆抽象来自 `wjsm-gc`；本模块保留 host 接合点
//! （active collect、roots、HeapAccessV2、GcContext 等）。
//!
//! active collect 按 `GcAlgorithmKind` 分派到 `active_v2` / `active_zgc`；
//! 对象堆唯一 owner 为 shared memory64 `HeapAccessV2`。
//!
//! 关键不变量见 v2 spec §22。

// ── host 接合点 ──
pub(crate) mod active_v2;
pub(crate) mod active_zgc;
pub mod api;
pub mod context;
pub mod heap_access;
mod heap_access_v2;
pub mod heap_governance;
pub mod native_callable_refs;
pub mod object_walker;
pub mod roots;
mod roots_v2;
pub mod scheduler;
pub mod side_table_refs;
pub mod weak_refs;

// host-only ZGC 接合（host_roots / V1 mark+relocate 协议保留）
pub mod zgc;

// ── 后端无关算法 / 控制面：re-export wjsm-gc ──
// 这些 re-export 供 crate 外部与 `lib.rs` 路径使用，本模块内未必直接引用。
#[allow(unused_imports)]
pub use wjsm_gc::api::{CycleKind, GcStats, Handle, StepBudget, Value};
#[allow(unused_imports)]
pub use wjsm_gc::{
    CollectorContext, GcAlgorithmKind, GcPacketKind, GcRuntimeV2, GcTelemetry, GcTelemetrySnapshot,
    GcWorkPacket, GcWorkerPool, HistogramSnapshot, MarkBitmap, MutatorContext, RootSnapshot,
    WorkerPoolError, WorkerPoolStats, GC_TELEMETRY_SCHEMA_VERSION, g1, mark_sweep, telemetry,
    worker,
};

// ── host-only 接合导出 ──
pub use api::GcContext;
pub use heap_access_v2::{HeapAccessV2, HeapAccessV2Error, HeapAccessV2Property};
pub use roots_v2::V2ConditionalRoots;

// 兼容旧路径（crate::runtime_gc::{control,registry,...}）
pub mod registry {
    #[allow(unused_imports)]
    pub use wjsm_gc::GcAlgorithmKind;
}
pub mod mark_bitmap {
    #[allow(unused_imports)]
    pub use wjsm_gc::MarkBitmap;
}
pub mod cpu_time {
    #[allow(unused_imports)]
    pub use wjsm_gc::thread_cpu_ns;
}
pub mod control {
    #[allow(unused_imports)]
    pub use wjsm_gc::{GcRuntimeV2, RootSnapshot};
}
pub mod collector_context {
    #[allow(unused_imports)]
    pub use wjsm_gc::CollectorContext;
}
pub mod mutator {
    #[allow(unused_imports)]
    pub use wjsm_gc::MutatorContext;
}
