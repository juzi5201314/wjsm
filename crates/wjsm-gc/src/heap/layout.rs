//! managed heap 的地址空间布局（后端无关纯数据）。

use super::handle_entry::{
    ADDRESS_LIMIT, HANDLE_REGION_BYTES, HEAP_COMMIT_GRANULE_BYTES, HandleTableError,
};

/// managed heap 的地址布局：handle region + control 区 + object heap 区。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedHeapLayout {
    control_base: u64,
    control_end: u64,
    object_heap_base: u64,
    object_heap_end: u64,
}

impl ManagedHeapLayout {
    pub fn new(max_heap_size: u64, control_reserved: u64) -> Result<Self, HandleTableError> {
        let control_base = HANDLE_REGION_BYTES;
        let control_end = align_up(
            control_base
                .checked_add(control_reserved)
                .ok_or(HandleTableError::LayoutOverflow)?,
        )?;
        let object_heap_end = control_end
            .checked_add(max_heap_size)
            .ok_or(HandleTableError::LayoutOverflow)?;
        if object_heap_end > ADDRESS_LIMIT {
            return Err(HandleTableError::LayoutExceedsAddressSpace { object_heap_end });
        }
        Ok(Self {
            control_base,
            control_end,
            object_heap_base: control_end,
            object_heap_end,
        })
    }

    pub const fn control_base(&self) -> u64 {
        self.control_base
    }

    pub const fn control_end(&self) -> u64 {
        self.control_end
    }

    pub const fn object_heap_base(&self) -> u64 {
        self.object_heap_base
    }

    pub const fn object_heap_end(&self) -> u64 {
        self.object_heap_end
    }

    pub(crate) fn contains_object_address(&self, address: u64) -> bool {
        (self.object_heap_base..self.object_heap_end).contains(&address)
    }
}

fn align_up(value: u64) -> Result<u64, HandleTableError> {
    let remainder = value % HEAP_COMMIT_GRANULE_BYTES;
    if remainder == 0 {
        Ok(value)
    } else {
        value
            .checked_add(HEAP_COMMIT_GRANULE_BYTES - remainder)
            .ok_or(HandleTableError::LayoutOverflow)
    }
}
