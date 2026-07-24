//! 后端无关的 GC 算法与堆抽象。
//!
//! 本 crate 提供 JS runtime 的垃圾回收算法（mark-sweep / G1 / ZGC）与
//! managed heap / handle table 抽象，**不依赖** wasmtime 或任何具体后端。
//! 后端经 [`heap::HeapMemory`] / [`heap::GrowableHeapMemory`] trait 提供堆存储，
//! 算法经泛型单态化于该接口。
//!
//! # 与 wjsm-host-wasm 的关系
//!
//! GC **算法**（本 crate）与 GC **接合点**（host-wasm 的 `runtime_gc::active_*`、
//! 根扫描、HeapAccessV2）分离：算法只消费后端无关的 `RootSnapshot` 与
//! `HeapMemory`，接合点负责从 `RuntimeState`/wasm 内存收集根并驱动算法。

pub mod api;
pub mod heap;
pub mod mark_bitmap;
pub mod mark_sweep;
pub mod g1;
pub mod telemetry;
pub mod zgc;

pub mod collector_context;
pub mod control;
pub mod cpu_time;
pub mod mutator;
pub mod registry;
pub mod worker;

pub use api::{
    CycleKind, GcExecutionStats, GcStats, Handle, MemoryFootprintSample, StepBudget, Value,
};
pub use collector_context::CollectorContext;
pub use cpu_time::thread_cpu_ns;
pub use control::{GcRuntimeV2, RootSnapshot};
pub use g1::{G1V2, G1V2CollectionKind, G1V2Error, G1V2Generation, G1V2Report};
pub use heap::{
    Allocation, AllocationClass, AllocatorError, GrowableHeapMemory, HANDLE_ENTRY_BYTES,
    HANDLE_REGION_BYTES, HandleGeneration, HandleId, HandleState, HandleTableError, HandleTableV2,
    HeapAddress, HeapMemory, HeapMemoryError, ManagedAllocator, ManagedHeap, ManagedHeapLayout,
    NativeHeapMemory, Nlab, ObjectRef, PAGE_GRANULE_BYTES,
};
pub use mark_bitmap::MarkBitmap;
pub use mark_sweep::{MarkSweepV2, MarkSweepV2Allocation, MarkSweepV2Error, MarkSweepV2Report};
pub use mutator::MutatorContext;
pub use registry::GcAlgorithmKind;
pub use telemetry::{
    GC_TELEMETRY_SCHEMA_VERSION, GcTelemetry, GcTelemetrySnapshot, HistogramSnapshot,
};
pub use worker::{GcPacketKind, GcWorkPacket, GcWorkerPool, WorkerPoolError, WorkerPoolStats};
pub use zgc::{
    BarrierEpoch, BarrierRecord, BarrierRing, BulkCopyMode, ConcurrentRelocator, HeaderFieldKind,
    HeaderLayout, LoadBarrierOutcome, OldController, OldPhase, RelocationDescriptor, YoungController,
    YoungPhase, ZgcV2, ZgcV2Error, ZgcV2Phase, ZgcV2Report, ZgcV2StepOutcome, color_stored_value,
    load_barrier, prototype_field_kind, publish_promotion, select_bulk_copy_mode, store_barrier,
    store_barrier_with_target_generation,
};
