//! Generational ZGC：算法来自 `wjsm-gc`，host 接合保留。
//!
//! - 算法：barrier / color / concurrent_relocate / director / old / page / remset / young / v2
//! - host-only：`host_roots`（Realm / V2ConditionalRoots）
//! - 协议保留：`mark` / `relocate`（单元测试；active path 不构造）

// ── wjsm-gc 算法 re-export ──
#[allow(unused_imports)]
pub use wjsm_gc::zgc::{
    barrier, color, concurrent_relocate, director, old, page, remset, young,
};
pub use wjsm_gc::zgc::{
    AssistBudget, BarrierEpoch, BarrierRecord, BarrierRing, BulkCopyMode, ConcurrentRelocator,
    DirectorDecision, DirectorGeneration, GcDirector, GenerationRates, HeaderField, HeaderFieldKind,
    HeaderLayout, LoadBarrierOutcome, OldController, OldPhase, OldReport, PageRelocationState,
    PreciseRemset, RelocationDescriptor, RelocationReport, StallEvent, StallReason, YoungController,
    YoungPhase, YoungReport, ZgcV2, ZgcV2Error, ZgcV2Phase, ZgcV2Report, ZgcV2StepOutcome,
    classify_entry, color_stored_value, load_barrier, prototype_field_kind, publish_promotion,
    select_bulk_copy_mode, store_barrier, store_barrier_with_target_generation,
};

// ── host-only ──
pub mod host_roots;
/// V1 colored-handle mark 状态机（单元测试与协议保留；active path 不构造）。
#[allow(dead_code)]
mod mark;
/// V1 relocate 状态机（单元测试与协议保留；active path 不构造）。
#[allow(dead_code)]
mod relocate;

pub use host_roots::{ConcurrentHostRoots, HostRootsReport, WeakState};
