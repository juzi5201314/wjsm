pub const PAGE_GRANULE_BYTES: u64 = 64 * 1024;
const MIB: u64 = 1024 * 1024;

/// heap-relative object identity；绝不承载 host pointer。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ObjectRef(u64);

impl ObjectRef {
    pub(crate) const fn new(offset: u64) -> Self {
        Self(offset)
    }

    pub const fn offset(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PageId(u32);

impl PageId {
    pub(crate) const fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub(crate) const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationClass {
    Small,
    Medium,
    Large,
    Humongous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageRange {
    start: PageId,
    len: u32,
}

impl PageRange {
    pub(crate) const fn new(start: PageId, len: u32) -> Self {
        Self { start, len }
    }

    pub const fn start(self) -> PageId {
        self.start
    }

    pub const fn len(self) -> u32 {
        self.len
    }

    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    pub const fn overlaps(self, other: Self) -> bool {
        let end = self.start.get() as u64 + self.len as u64;
        let other_end = other.start.get() as u64 + other.len as u64;
        (self.start.get() as u64) < other_end && (other.start.get() as u64) < end
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PageConfig {
    pub(crate) bytes: u64,
    pub(crate) small_limit: u64,
    pub(crate) medium_limit: u64,
    pub(crate) large_limit: u64,
}

impl PageConfig {
    pub(crate) fn for_heap(max_heap_bytes: u64) -> Result<Self, &'static str> {
        if max_heap_bytes < PAGE_GRANULE_BYTES {
            return Err("heap is smaller than one page granule");
        }
        let bytes = if max_heap_bytes <= 256 * MIB {
            PAGE_GRANULE_BYTES
        } else if max_heap_bytes <= 4 * 1024 * MIB {
            256 * 1024
        } else if max_heap_bytes <= 16 * 1024 * MIB {
            MIB
        } else {
            2 * MIB
        };
        Ok(Self {
            bytes,
            small_limit: bytes / 4,
            medium_limit: bytes.saturating_mul(16),
            large_limit: 32 * MIB,
        })
    }
}
