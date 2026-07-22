use std::error::Error;
use std::fmt;

pub const HANDLE_ENTRY_BYTES: u64 = 8;
pub const HANDLE_REGION_BYTES: u64 = 32 * 1024 * 1024 * 1024;
pub const HEAP_COMMIT_GRANULE_BYTES: u64 = 64 * 1024;

pub(crate) const ADDRESS_LIMIT: u64 = 1_u64 << 48;
const STATE_MASK: u64 = u16::MAX as u64;

/// memory64 ABI 中保持不变的 JavaScript handle identity。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HandleId(u32);

impl HandleId {
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// 对象所在世代；entry 的具体状态仍由 `HandleState` 表示。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandleGeneration {
    Young,
    Old,
}

/// 与 memory64 ABI 对齐的低 16-bit handle entry 状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum HandleState {
    Free = 0,
    StableYoung = 1,
    StableOld = 2,
    RelocatingYoung = 3,
    RelocatingOld = 4,
    PinnedOld = 5,
    Retired = 6,
}

impl HandleState {
    pub(crate) const fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            0 => Some(Self::Free),
            1 => Some(Self::StableYoung),
            2 => Some(Self::StableOld),
            3 => Some(Self::RelocatingYoung),
            4 => Some(Self::RelocatingOld),
            5 => Some(Self::PinnedOld),
            6 => Some(Self::Retired),
            _ => None,
        }
    }

    pub(crate) const fn stable_for(generation: HandleGeneration) -> Self {
        match generation {
            HandleGeneration::Young => Self::StableYoung,
            HandleGeneration::Old => Self::StableOld,
        }
    }

    pub(crate) const fn relocating_for(generation: HandleGeneration) -> Self {
        match generation {
            HandleGeneration::Young => Self::RelocatingYoung,
            HandleGeneration::Old => Self::RelocatingOld,
        }
    }

    pub(crate) const fn generation(self) -> Option<HandleGeneration> {
        match self {
            Self::StableYoung | Self::RelocatingYoung => Some(HandleGeneration::Young),
            Self::StableOld | Self::RelocatingOld | Self::PinnedOld => Some(HandleGeneration::Old),
            Self::Free | Self::Retired => None,
        }
    }

    pub(crate) const fn is_stable(self) -> bool {
        matches!(self, Self::StableYoung | Self::StableOld | Self::PinnedOld)
    }
}

/// 高 48 bit 为 byte address、低 16 bit 为状态的不可变 entry 快照。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColoredHandleEntry(u64);

impl ColoredHandleEntry {
    pub(crate) fn new(address: u64, state: HandleState) -> Result<Self, HandleTableError> {
        if address >= ADDRESS_LIMIT {
            return Err(HandleTableError::AddressOutOfRange { address });
        }
        if !matches!(state, HandleState::Free) && !address.is_multiple_of(HANDLE_ENTRY_BYTES) {
            return Err(HandleTableError::UnalignedAddress { address });
        }
        Ok(Self((address << 16) | u64::from(state as u16)))
    }

    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn address(self) -> u64 {
        self.0 >> 16
    }

    pub fn state(self) -> HandleState {
        let raw = (self.0 & STATE_MASK) as u16;
        HandleState::from_raw(raw).expect("invalid handle entry state")
    }

    pub fn generation(self) -> HandleGeneration {
        self.state()
            .generation()
            .expect("non-live handle entry has no generation")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HandleTableError {
    AddressOutOfRange {
        address: u64,
    },
    AddressOutsideObjectHeap {
        address: u64,
    },
    HandleExhausted,
    InvalidTransition {
        handle: HandleId,
        expected: HandleState,
        actual: HandleState,
    },
    LayoutExceedsAddressSpace {
        object_heap_end: u64,
    },
    LayoutOverflow,
    UnalignedAddress {
        address: u64,
    },
    VirtualReservation {
        detail: String,
    },
}

impl fmt::Display for HandleTableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AddressOutOfRange { address } => {
                write!(formatter, "handle address {address:#x} exceeds 48-bit ABI")
            }
            Self::AddressOutsideObjectHeap { address } => {
                write!(formatter, "address {address:#x} is outside object heap")
            }
            Self::HandleExhausted => formatter.write_str("handle table is exhausted"),
            Self::InvalidTransition {
                handle,
                expected,
                actual,
            } => write!(
                formatter,
                "handle {} transition expected {expected:?}, found {actual:?}",
                handle.get()
            ),
            Self::LayoutExceedsAddressSpace { object_heap_end } => write!(
                formatter,
                "object heap end {object_heap_end:#x} exceeds 48-bit ABI"
            ),
            Self::LayoutOverflow => formatter.write_str("managed heap layout overflows u64"),
            Self::UnalignedAddress { address } => write!(
                formatter,
                "handle address {address:#x} is not 8-byte aligned"
            ),
            Self::VirtualReservation { detail } => {
                write!(
                    formatter,
                    "unable to reserve 32 GiB handle region: {detail}"
                )
            }
        }
    }
}

impl Error for HandleTableError {}
