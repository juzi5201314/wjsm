//! MarkSweep V2：`ManagedHeap<M: GrowableHeapMemory>` 上的非移动完整回收。
//!
//! 本算法已后端无关：经 `GrowableHeapMemory` 单态化，不依赖具体执行后端。

mod v2;
pub use v2::{MarkSweepV2, MarkSweepV2Allocation, MarkSweepV2Error, MarkSweepV2Report};
