//! 后端无关的 GC 算法与堆抽象。
//!
//! 本 crate 提供 JS runtime 的垃圾回收算法（mark-sweep / G1 / ZGC）与
//! managed heap / handle table 抽象；算法不依赖具体执行后端。
//! 后端经 [`heap::HeapMemory`] / [`heap::GrowableHeapMemory`] trait 提供堆存储，
//! 算法经泛型单态化于该接口。
//!
//! roots、safepoint 与 native frame 的接合由 `wjsm-host-native` 负责；本 crate
//! 只消费后端无关的 `RootSnapshot` 与 `HeapMemory`。

pub mod api;

pub mod g1;
pub mod heap;
pub mod heap_access;
pub mod mark_sweep;
pub mod property_key;
pub mod shape;
pub mod stop_the_world;
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
pub use control::{GcEdge, GcEphemeron, GcRuntimeV2, RootSnapshot};
pub use cpu_time::thread_cpu_ns;
pub use g1::{G1V2, G1V2CollectionKind, G1V2Error, G1V2Generation, G1V2Report};
pub use heap::{
    Allocation, AllocationClass, AllocatorError, GrowableHeapMemory, HANDLE_ENTRY_BYTES,
    HANDLE_REGION_BYTES, HANDLE_STATE_STABLE_MIN, HandleGeneration, HandleId, HandleState,
    HandleTableError, HandleTableV2, HeapAddress, HeapEpoch, HeapMemory, HeapMemoryError,
    ManagedAllocator, ManagedHeap, ManagedHeapLayout, NativeHeapMemory, Nlab, ObjectRef,
    PAGE_GRANULE_BYTES, PageStats, RelocationNlab, TestHeapMemory,
};
pub use heap_access::{
    CollectorHeapCapability, HeapAccessV2, HeapAccessV2Error, HeapAccessV2Property,
};
pub use mark_sweep::{MarkSweepV2, MarkSweepV2Allocation, MarkSweepV2Error, MarkSweepV2Report};
pub use mutator::MutatorContext;
pub use property_key::PropertyKey;
pub use registry::GcAlgorithmKind;
pub use shape::{PROTO_NULL_SENTINEL, ShapeProp, ShapeTable, ShapeTableSnapshot, ShapeTransition};
pub use stop_the_world::{RuntimeGcReport, StopTheWorldCollector, StopTheWorldCollectorError};
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
