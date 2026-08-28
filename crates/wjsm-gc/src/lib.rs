//! 后端无关的 GC 算法与堆抽象。
//!
//! 本 crate 提供 JS runtime 的并发分代 ZGC 与 managed heap / handle table 抽象；
//! 算法不依赖具体执行后端。后端经 [`heap::HeapMemory`] / [`heap::GrowableHeapMemory`]
//! trait 提供堆存储，算法经泛型单态化于该接口。
//!
//! roots、safepoint 与 native frame 的接合由 `wjsm-host-native` 负责；本 crate
//! 只消费后端无关的 `RootSnapshot` 与 `HeapMemory`。

pub mod backoff;

pub mod api;

pub mod heap;
pub mod heap_access;
pub mod property_key;
pub mod shape;
pub mod telemetry;
pub mod zgc;

pub mod string_view;

pub mod collector_context;
pub mod control;
pub mod cpu_time;
pub mod mutator;
pub mod worker;

pub use api::{
    CycleKind, GcExecutionStats, GcStats, Handle, MemoryFootprintSample, RuntimeGcReport,
    StepBudget, Value,
};
pub use collector_context::CollectorContext;
pub use control::{GcEdge, GcEphemeron, GcRuntimeV2, RootSnapshot};
pub use cpu_time::thread_cpu_ns;
pub use heap::{
    Allocation, AllocationClass, AllocatorError, GrowableHeapMemory, HANDLE_ENTRY_BYTES,
    HANDLE_REGION_BYTES, HANDLE_STATE_STABLE_MIN, HandleGeneration, HandleId,
    HandleRangeReservation, HandleState, HandleTableError, HandleTableV2, HeapAddress, HeapEpoch,
    HeapMemory, HeapMemoryError, ManagedAllocator, ManagedHeap, ManagedHeapLayout,
    NativeHeapMemory, NativeTlabReservation, Nlab, ObjectRef, PAGE_GRANULE_BYTES, PageStats,
    RelocationNlab, TestHeapMemory,
};

pub use heap_access::{
    CollectorHeapCapability, HeapAccessV2, HeapAccessV2Error, HeapAccessV2Property,
    object_payload_bytes,
};
pub use mutator::MutatorContext;
pub use property_key::PropertyKey;
pub use shape::{
    PROTO_NULL_SENTINEL, PROTO_PROXY_FLAG, PROTO_REGEXP_FLAG, ShapeProp, ShapeTable,
    ShapeTableSnapshot, ShapeTransition, proto_slot_is_exotic,
};
pub use string_view::StrView;
pub use telemetry::{
    GC_TELEMETRY_SCHEMA_VERSION, GcTelemetry, GcTelemetrySnapshot, HistogramSnapshot,
};
pub use worker::{GcPacketKind, GcWorkPacket, GcWorkerPool, WorkerPoolError, WorkerPoolStats};
pub use zgc::{
    BarrierEpoch, BarrierRecord, BarrierRing, BulkCopyMode, ConcurrentRelocator, GcSafepointAction,
    GenerationalZgc, GenerationalZgcError, HeaderFieldKind, HeaderLayout, HeapBarrier,
    LoadBarrierOutcome, RelocationDescriptor, ZgcBarrierSet, color_stored_value, load_barrier,
    prototype_field_kind, publish_promotion, select_bulk_copy_mode, store_barrier,
    store_barrier_with_target_generation,
};
