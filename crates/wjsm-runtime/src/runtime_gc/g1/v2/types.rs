use std::error::Error;
use std::fmt;

use crate::heap::{AllocatorError, HandleId, HandleTableError};
use crate::runtime_gc::WorkerPoolError;

use super::super::region::RegionKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum G1V2Generation {
    Eden,
    Survivor,
    Old,
    Humongous,
}

impl G1V2Generation {
    pub(super) fn is_young(self) -> bool {
        matches!(self, Self::Eden | Self::Survivor)
    }

    pub(super) fn region_kind(self) -> RegionKind {
        match self {
            Self::Eden => RegionKind::Eden,
            Self::Survivor => RegionKind::Survivor,
            Self::Old => RegionKind::Old,
            Self::Humongous => RegionKind::HumongousStart,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum G1V2CollectionKind {
    Young,
    Mixed,
    Full,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct G1V2Report {
    pub kind: Option<G1V2CollectionKind>,
    pub marked: usize,
    pub evacuated: usize,
    pub relocated_bytes: u64,
    pub promoted: usize,
    pub promoted_bytes: u64,
    pub retired: usize,
    pub reclaimed_bytes: u64,
    pub reclaimed_pages: u32,
    pub remembered_cards_scanned: usize,
    pub promotion_failed: bool,
}

#[derive(Debug)]
pub enum G1V2Error {
    Allocation(AllocatorError),
    Handle(HandleTableError),
    Memory(String),
    UnknownHandle(HandleId),
    Worker(WorkerPoolError),
}

impl fmt::Display for G1V2Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allocation(error) => write!(formatter, "G1 V2 allocation failed: {error}"),
            Self::Handle(error) => write!(formatter, "G1 V2 handle failed: {error}"),
            Self::Memory(error) => write!(formatter, "G1 V2 memory failed: {error}"),
            Self::UnknownHandle(handle) => {
                write!(formatter, "G1 V2 handle {} is not tracked", handle.get())
            }
            Self::Worker(error) => write!(formatter, "G1 V2 worker failed: {error}"),
        }
    }
}

impl Error for G1V2Error {}

impl From<AllocatorError> for G1V2Error {
    fn from(error: AllocatorError) -> Self {
        Self::Allocation(error)
    }
}

impl From<HandleTableError> for G1V2Error {
    fn from(error: HandleTableError) -> Self {
        Self::Handle(error)
    }
}

impl From<WorkerPoolError> for G1V2Error {
    fn from(error: WorkerPoolError) -> Self {
        Self::Worker(error)
    }
}
