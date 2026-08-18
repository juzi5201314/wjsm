//! G1 V2：region/card remembered-set 元数据 + 后端无关收集。
//!
//! 本算法经 `GrowableHeapMemory` 单态化，不依赖具体执行后端。

mod region;
mod rset;
mod v2;

pub use v2::{G1V2, G1V2CollectionKind, G1V2Error, G1V2Generation, G1V2Report};
