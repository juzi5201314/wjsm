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
///
/// 判别值刻意让稳定态成为连续高值区间（>= [`HANDLE_STATE_STABLE_MIN`]），
/// 让生成代码可用一次无符号比较判断句柄是否可直接访问。
///
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum HandleState {
    Free = 0,
    Retired = 1,
    RelocatingYoung = 2,
    RelocatingOld = 3,
    /// 稳定态起点：以下状态的 entry 地址可被直接使用。
    StableYoung = 4,
    StableOld = 5,
    PinnedOld = 6,
}
/// 稳定态判别值下界；生成代码用 `state >= HANDLE_STATE_STABLE_MIN` 单比较判定。
pub const HANDLE_STATE_STABLE_MIN: u16 = HandleState::StableYoung as u16;

// 状态编码是 codegen（`wjsm_ir::constants::HANDLE_STATE_*`）与本模块之间的 ABI 契约。
// 两处必须逐一相等，否则生成代码会把对象判为非稳定态；用编译期断言钉死。
const _: () = {
    use wjsm_ir::constants as abi;
    assert!(HandleState::Free as u16 as u32 == abi::HANDLE_STATE_FREE);
    assert!(HandleState::Retired as u16 as u32 == abi::HANDLE_STATE_RETIRED);
    assert!(HandleState::RelocatingYoung as u16 as u32 == abi::HANDLE_STATE_RELOCATING_YOUNG);
    assert!(HandleState::RelocatingOld as u16 as u32 == abi::HANDLE_STATE_RELOCATING_OLD);
    assert!(HandleState::StableYoung as u16 as u32 == abi::HANDLE_STATE_STABLE_YOUNG);
    assert!(HandleState::StableOld as u16 as u32 == abi::HANDLE_STATE_STABLE_OLD);
    assert!(HandleState::PinnedOld as u16 as u32 == abi::HANDLE_STATE_PINNED_OLD);
    assert!(HANDLE_STATE_STABLE_MIN as u32 == abi::HANDLE_STATE_STABLE_MIN);
};

// `stable_for`/`relocating_for`/`is_stable` 等由 HandleTableV2 使用；随 GC 算法层迁移后启用。
#[allow(dead_code)]
impl HandleState {
    pub const fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            0 => Some(Self::Free),
            1 => Some(Self::Retired),
            2 => Some(Self::RelocatingYoung),
            3 => Some(Self::RelocatingOld),
            4 => Some(Self::StableYoung),
            5 => Some(Self::StableOld),
            6 => Some(Self::PinnedOld),
            _ => None,
        }
    }

    pub const fn stable_for(generation: HandleGeneration) -> Self {
        match generation {
            HandleGeneration::Young => Self::StableYoung,
            HandleGeneration::Old => Self::StableOld,
        }
    }

    pub const fn relocating_for(generation: HandleGeneration) -> Self {
        match generation {
            HandleGeneration::Young => Self::RelocatingYoung,
            HandleGeneration::Old => Self::RelocatingOld,
        }
    }

    pub const fn generation(self) -> Option<HandleGeneration> {
        match self {
            Self::StableYoung | Self::RelocatingYoung => Some(HandleGeneration::Young),
            Self::StableOld | Self::RelocatingOld | Self::PinnedOld => Some(HandleGeneration::Old),
            Self::Free | Self::Retired => None,
        }
    }

    pub const fn is_stable(self) -> bool {
        (self as u16) >= HANDLE_STATE_STABLE_MIN
    }
}

/// 高 48 bit 为 byte address、低 16 bit 为状态的不可变 entry 快照。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColoredHandleEntry(u64);

// `new`/`from_raw`/`stable_for` 等由 HandleTableV2 使用；HandleTableV2 随 GC 算法层迁移后启用。
#[allow(dead_code)]
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
    DuplicateRestoreHandle {
        handle: HandleId,
    },
    HandleExhausted,
    UnallocatedHandle {
        handle: HandleId,
    },
    InvalidTransition {
        handle: HandleId,
        expected: HandleState,
        actual: HandleState,
    },
    LayoutExceedsAddressSpace {
        object_heap_end: u64,
    },
    LayoutOverflow,
    RestoreHandleOutOfRange {
        handle: HandleId,
        next_handle: u64,
    },
    RestoreRequiresEmpty,
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
            Self::DuplicateRestoreHandle { handle } => write!(
                formatter,
                "snapshot contains duplicate handle {}",
                handle.get()
            ),
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
            Self::RestoreHandleOutOfRange {
                handle,
                next_handle,
            } => write!(
                formatter,
                "snapshot handle {} is outside next_handle {next_handle}",
                handle.get()
            ),
            Self::RestoreRequiresEmpty => {
                formatter.write_str("snapshot restore requires an empty handle table")
            }
            Self::UnalignedAddress { address } => write!(
                formatter,
                "handle address {address:#x} is not 8-byte aligned"
            ),
            Self::UnallocatedHandle { handle } => {
                write!(
                    formatter,
                    "handle {} was not allocated by HandleTableV2",
                    handle.get()
                )
            }
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
