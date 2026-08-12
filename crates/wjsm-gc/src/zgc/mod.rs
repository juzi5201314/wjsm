//! Generational ZGC V2 协议与算法（后端无关）。
//!
//! 本模块只含可泛型化、不绑具体执行后端的部分；collector host 接合位于
//! `wjsm-host-native`。

pub mod barrier;
pub mod color;
pub mod concurrent_relocate;
pub mod director;
pub mod old;
pub mod page;
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
pub use old::{OldController, OldPhase, OldReport};
pub use remset::{PreciseRemset, publish_promotion};
pub use v2::{ZgcV2, ZgcV2Error, ZgcV2Phase, ZgcV2Report, ZgcV2StepOutcome};
pub use young::{YoungController, YoungPhase, YoungReport};
