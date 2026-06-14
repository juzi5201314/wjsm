//! 可插拔 GC 框架（spec §6）。单一 canonical owner: 本模块组。
//!
//! 算法以 trait 抽象（`GcAlgorithm: Allocator + Marker + Sweeper`），
//! 默认实现 `MarkSweepCollector`（non-moving + segregated free list）。
//! 预留 generational/incremental/parallel 接入点（WriteBarrier/ReadBarrier/
//! HeapRegionManager/mark_step/sweep_step，默认 no-op）。
//!
//! 稳定性承诺见 spec 附录 D。关键不变量见 spec §18。
pub mod api;
pub mod context;
pub mod mark_bitmap;
pub mod mark_sweep;
pub mod roots;

pub use api::{
    Allocator, GcAlgorithm, GcContext, GcStats, Handle, HeapObjectQuery, HeapRegionManager,
    Marker, MarkProgress, ReadBarrier, RootProvider, Sweeper, Value, WriteBarrier,
};
pub use mark_bitmap::MarkBitmap;
pub use mark_sweep::MarkSweepCollector;
pub use mark_sweep::allocator::SegregatedFreeList;
