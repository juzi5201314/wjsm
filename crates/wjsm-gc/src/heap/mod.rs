//! 后端无关的堆内存与 handle 抽象。
//!
//! 本模块是 GC 算法与 native host 堆之间的接缝。`HeapMemory`/
//! `GrowableHeapMemory` 由 native 后端实现，算法经泛型单态化，不绑定具体后端。

// 以下子模块是 GC 算法层（HandleTableV2 / MarkSweepV2 / G1V2 / ZgcV2）的预留依赖；
// 算法层迁完后它们被完全使用。当前仅 heap 纯模块落地，故暂允 dead_code。
#[allow(dead_code)]
mod allocator;
#[allow(dead_code)]
mod bitmap;
#[allow(dead_code)]
mod epoch;
mod handle;
#[allow(dead_code)]
mod handle_entry;
mod layout;
mod memory;
mod native_memory;
#[allow(dead_code)]
mod object_map;
#[allow(dead_code)]
mod page;
pub mod platform;
mod word;

pub use allocator::{Allocation, AllocatorError, ManagedAllocator, Nlab, RelocationReserve};
pub use epoch::EpochParticipant;
pub use handle::{HandleRegionBackend, HandleTableV2, PlatformHandleRegion, RestoredHandleEntry};
pub use handle_entry::{
    ColoredHandleEntry, HANDLE_ENTRY_BYTES, HANDLE_REGION_BYTES, HANDLE_STATE_STABLE_MIN,
    HandleGeneration, HandleId, HandleState, HandleTableError,
};
pub use layout::ManagedHeapLayout;
pub use memory::{GrowableHeapMemory, HeapMemory};
pub use native_memory::{NativeHeapMemory, TestHeapMemory};
pub use object_map::PageObjectIter;
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
        Ok(Self {
            memory,
            allocator: ManagedAllocator::new(layout)?,
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
}
