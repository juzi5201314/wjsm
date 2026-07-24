use std::error::Error;
use std::fmt;

use crate::heap::{AllocatorError, HandleId, HandleTableError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZgcV2Phase {
    Idle,
    Mark,
    Relocate,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ZgcV2Report {
    pub marked: usize,
    pub retired: usize,
    pub relocated: usize,
    pub relocation_deferred: usize,
    pub reclaimed_pages: u32,
    pub reclaimed_bytes: u64,
    pub relocated_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZgcV2StepOutcome {
    Progress { phase: ZgcV2Phase, remaining: usize },
    CycleComplete(ZgcV2Report),
}

#[derive(Debug)]
pub enum ZgcV2Error {
    Allocation(AllocatorError),
    Handle(HandleTableError),
    Memory(String),
    RelocationInProgress,
    UnknownHandle(HandleId),
}

impl fmt::Display for ZgcV2Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allocation(error) => write!(formatter, "ZGC V2 allocation failed: {error}"),
            Self::Handle(error) => write!(formatter, "ZGC V2 handle failed: {error}"),
            Self::Memory(error) => write!(formatter, "ZGC V2 memory failed: {error}"),
            Self::RelocationInProgress => {
                formatter.write_str("ZGC V2 references cannot change during relocation")
            }
            Self::UnknownHandle(handle) => {
                write!(formatter, "ZGC V2 handle {} is not tracked", handle.get())
            }
        }
    }
}

impl Error for ZgcV2Error {}

impl From<AllocatorError> for ZgcV2Error {
    fn from(error: AllocatorError) -> Self {
        Self::Allocation(error)
    }
}

impl From<HandleTableError> for ZgcV2Error {
    fn from(error: HandleTableError) -> Self {
        Self::Handle(error)
    }
}
