//! G1 V2：region/card remembered-set 元数据 + shared memory64 收集。
//!
//! V1 memory32 `G1Collector` 与 young/mixed/concurrent_mark 路径已退役。

#[allow(dead_code)]
mod region;
#[allow(dead_code)]
mod rset;
mod v2;

pub use v2::{G1V2, G1V2CollectionKind, G1V2Error, G1V2Generation, G1V2Report};

// region/rset 供 V2 与单元测试使用
#[allow(unused_imports)]
pub(crate) use region::{CARD_SIZE, REGION_SIZE, RegionKind, RegionSpace};
#[allow(unused_imports)]
pub(crate) use rset::{BarrierEvent, G1RSet, SlotOwner};
