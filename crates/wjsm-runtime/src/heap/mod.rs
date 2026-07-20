mod allocator;
mod bitmap;
mod epoch;
mod handle;
mod handle_entry;
mod memory;
mod native_memory;
mod object_map;
mod page;
pub mod platform;

mod word;

pub use allocator::{Allocation, AllocatorError, ManagedAllocator, Nlab, RelocationReserve};
pub use epoch::EpochParticipant;
pub use handle::{HandleTableV2, ManagedHeapLayout};
pub use handle_entry::{
    ColoredHandleEntry, HANDLE_ENTRY_BYTES, HANDLE_REGION_BYTES, HandleGeneration, HandleId,
    HandleState, HandleTableError,
};
pub use memory::SharedHeapMemory;
pub use native_memory::NativeHeapMemory;
pub use object_map::PageObjectIter;
pub use page::{AllocationClass, ObjectRef, PAGE_GRANULE_BYTES, PageId, PageRange};
pub use platform::{
    IsaDispatch, IsaKind, NumaNode, NumaTopology, PlatformCapabilities, PlatformError,
    PlatformVirtualMemory, ScalarBitmapOps, VirtualRange, reserve as platform_reserve,
    set_thread_affinity,
};
pub use word::{HeapAddress, HeapMemoryError};

use memory::HeapMemory;

/// 单态化 managed heap owner；生产路径不会使用 trait object。
// Task 4 只建立类型边界；active Linker 在 Task 15 前不会构造此 owner。
#[allow(dead_code)]
pub(crate) struct ManagedHeap<M> {
    memory: M,
    allocator: ManagedAllocator,
}

#[allow(dead_code)]
impl<M> ManagedHeap<M> {
    pub(crate) fn new(memory: M, layout: ManagedHeapLayout) -> Result<Self, AllocatorError> {
        Ok(Self {
            memory,
            allocator: ManagedAllocator::new(layout)?,
        })
    }

    pub(crate) fn memory(&self) -> &M {
        &self.memory
    }

    pub(crate) fn allocate(
        &self,
        nlab: &mut Nlab,
        bytes: u64,
    ) -> Result<Allocation, AllocatorError> {
        self.allocator.allocate(nlab, bytes)
    }

    pub(crate) fn allocator(&self) -> &ManagedAllocator {
        &self.allocator
    }
}

#[allow(dead_code)]
impl<M: HeapMemory> ManagedHeap<M> {
    pub(crate) fn byte_len(&self) -> u64 {
        self.memory.byte_len()
    }

    pub(crate) fn load_word(&self, address: HeapAddress) -> Result<u64, HeapMemoryError> {
        self.memory.load_word(address)
    }

    pub(crate) fn store_word(
        &self,
        address: HeapAddress,
        value: u64,
    ) -> Result<(), HeapMemoryError> {
        self.memory.store_word(address, value)
    }

    pub(crate) fn copy_from(
        &self,
        address: HeapAddress,
        bytes: &[u8],
    ) -> Result<(), HeapMemoryError> {
        self.memory.copy_from(address, bytes)
    }

    pub(crate) fn copy_to(
        &self,
        address: HeapAddress,
        length: u64,
    ) -> Result<Vec<u8>, HeapMemoryError> {
        self.memory.copy_to(address, length)
    }
}

/// active runtime 切换前的生产类型别名；Task 4 只定义，不接入 Linker。
#[allow(dead_code)]
pub(crate) type RuntimeManagedHeap = ManagedHeap<SharedHeapMemory>;

#[cfg(test)]
mod tests {
    use super::{ManagedHeap, ManagedHeapLayout, NativeHeapMemory, Nlab};

    #[test]
    fn managed_heap_delegates_nlab_allocation() {
        let layout = ManagedHeapLayout::new(64 * 1024, 64 * 1024).unwrap();
        let heap = ManagedHeap::new(NativeHeapMemory::with_base(0, 64), layout).unwrap();
        let mut nlab = Nlab::new();

        let allocation = heap.allocate(&mut nlab, 8).unwrap();

        assert_eq!(
            allocation.object().offset(),
            heap.allocator().layout().object_heap_base()
        );
        assert_eq!(nlab.refills(), 1);
    }
}
