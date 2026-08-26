//! 后端无关的堆内存与 handle 抽象。
//!
//! 本模块是 GC 算法与 native host 堆之间的接缝。`HeapMemory`/
//! `GrowableHeapMemory` 由 native 后端实现，算法经泛型单态化，不绑定具体后端。

use std::sync::Arc;

mod allocator;
mod bitmap;
mod epoch;
mod handle;
mod handle_entry;
mod layout;
mod memory;
mod native_memory;
mod object_map;
mod page;
pub mod platform;
mod word;

pub use allocator::{
    Allocation, AllocatorError, ManagedAllocator, NativeTlabReservation, Nlab, RelocationNlab,
};
pub use epoch::{EpochParticipant, HeapEpoch};
pub use handle::{
    HandleRangeReservation, HandleRegionBackend, HandleTableV2, PlatformHandleRegion,
    RestoredHandleEntry,
};
pub use handle_entry::{
    ColoredHandleEntry, HANDLE_ENTRY_BYTES, HANDLE_REGION_BYTES, HANDLE_STATE_STABLE_MIN,
    HandleGeneration, HandleId, HandleState, HandleTableError,
};
pub use layout::ManagedHeapLayout;
pub use memory::{GrowableHeapMemory, HeapMemory};
pub use native_memory::{NativeHeapMemory, TestHeapMemory};
pub use object_map::{PageObjectIter, PageStats};
pub use page::{AllocationClass, ObjectRef, PAGE_GRANULE_BYTES, PageId, PageRange};
pub use platform::{
    IsaDispatch, IsaKind, NumaNode, NumaTopology, PlatformCapabilities, PlatformError,
    PlatformVirtualMemory, ScalarBitmapOps, VirtualRange, reserve as platform_reserve,
    set_thread_affinity,
};
pub use word::{HeapAddress, HeapMemoryError};

/// 单态化 managed heap owner；生产路径不会使用 trait object。
pub struct ManagedHeap<M> {
    memory: M,
    allocator: ManagedAllocator,
}

impl<M> ManagedHeap<M> {
    pub fn new(memory: M, layout: ManagedHeapLayout) -> Result<Self, AllocatorError> {
        Self::with_epoch(memory, layout, HeapEpoch::new())
    }

    pub fn with_epoch(
        memory: M,
        layout: ManagedHeapLayout,
        epoch: Arc<HeapEpoch>,
    ) -> Result<Self, AllocatorError> {
        Ok(Self {
            memory,
            allocator: ManagedAllocator::with_epoch(layout, epoch)?,
        })
    }

    pub fn memory(&self) -> &M {
        &self.memory
    }

    pub fn allocate(&self, nlab: &mut Nlab, bytes: u64) -> Result<Allocation, AllocatorError> {
        self.allocator.allocate(nlab, bytes)
    }

    pub fn allocator(&self) -> &ManagedAllocator {
        &self.allocator
    }
}

impl<M: HeapMemory> ManagedHeap<M> {
    pub fn byte_len(&self) -> u64 {
        self.memory.byte_len()
    }

    pub fn load_word(&self, address: HeapAddress) -> Result<u64, HeapMemoryError> {
        self.memory.load_word(address)
    }

    pub fn store_word(&self, address: HeapAddress, value: u64) -> Result<(), HeapMemoryError> {
        self.memory.store_word(address, value)
    }

    pub fn copy_from(&self, address: HeapAddress, bytes: &[u8]) -> Result<(), HeapMemoryError> {
        self.memory.copy_from(address, bytes)
    }

    pub fn copy_to(&self, address: HeapAddress, length: u64) -> Result<Vec<u8>, HeapMemoryError> {
        self.memory.copy_to(address, length)
    }

    pub fn copy_nonoverlapping_unpublished(
        &self,
        source: HeapAddress,
        destination: HeapAddress,
        length: u64,
    ) -> Result<(), HeapMemoryError> {
        self.memory
            .copy_nonoverlapping_unpublished(source, destination, length)
    }

    pub fn copy_atomic_words(
        &self,
        source: HeapAddress,
        destination: HeapAddress,
        length: u64,
    ) -> Result<(), HeapMemoryError> {
        self.memory.copy_atomic_words(source, destination, length)
    }
}
