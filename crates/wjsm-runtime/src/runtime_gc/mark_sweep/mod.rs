//! MarkSweep V2：shared memory64 ManagedHeap 上的非移动完整回收。
//!
//! V1 memory32 `MarkSweepCollector` 已退役；active collect 经 `active_v2` 调度。

mod v2;
pub use v2::{MarkSweepV2, MarkSweepV2Allocation, MarkSweepV2Error, MarkSweepV2Report};
