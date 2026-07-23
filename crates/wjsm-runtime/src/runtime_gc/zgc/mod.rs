//! Generational ZGC V2 协议与 active 收集入口。
//!
//! V1 memory32 `ZgcCollector` / `alloc_from_bump` 已退役。
//! active full collect 经 `active_zgc::collect_dispatch` 调度。

pub mod barrier;
pub mod color;
pub mod concurrent_relocate;
pub mod director;
pub mod host_roots;
/// V1 colored-handle mark 状态机（单元测试与协议保留；active path 不构造）。
#[allow(dead_code)]
mod mark;
pub mod old;
pub mod page;
/// V1 relocate 状态机（单元测试与协议保留；active path 不构造）。
#[allow(dead_code)]
mod relocate;
pub mod remset;
mod v2;
pub mod young;

pub use barrier::{
    BarrierEpoch, BarrierRecord, BarrierRing, BulkCopyMode, HeaderField, HeaderFieldKind,
    HeaderLayout, LoadBarrierOutcome, classify_entry, color_stored_value, load_barrier,
    prototype_field_kind, select_bulk_copy_mode, store_barrier,
    store_barrier_with_target_generation,
};
pub use concurrent_relocate::{
    ConcurrentRelocator, PageRelocationState, RelocationDescriptor, RelocationReport,
};
pub use director::{
    AssistBudget, DirectorDecision, DirectorGeneration, GcDirector, GenerationRates, StallEvent,
    StallReason,
};
pub use host_roots::{ConcurrentHostRoots, HostRootsReport, WeakState};
pub use old::{OldController, OldPhase, OldReport};
pub use remset::{PreciseRemset, publish_promotion};
pub use v2::{ZgcV2, ZgcV2Error, ZgcV2Phase, ZgcV2Report, ZgcV2StepOutcome};
pub use young::{YoungController, YoungPhase, YoungReport};

