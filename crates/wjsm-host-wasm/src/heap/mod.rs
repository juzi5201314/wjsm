//! host-wasm 堆接缝：从 `wjsm-gc` re-export 后端无关堆类型，
//! 并保留 wasmtime 专有的 [`SharedHeapMemory`]。

mod memory;

pub use memory::SharedHeapMemory;
#[allow(unused_imports)]
pub use wjsm_gc::heap::{
    Allocation, AllocationClass, AllocatorError, ColoredHandleEntry, EpochParticipant,
    GrowableHeapMemory, HANDLE_ENTRY_BYTES, HANDLE_REGION_BYTES, HandleGeneration, HandleId,
    HandleRegionBackend, HandleState, HandleTableError, HandleTableV2, HeapAddress, HeapMemory,
    HeapMemoryError, IsaDispatch, IsaKind, ManagedAllocator, ManagedHeap, ManagedHeapLayout,
    NativeHeapMemory, Nlab, NumaNode, NumaTopology, ObjectRef, PAGE_GRANULE_BYTES, PageId,
    PageObjectIter, PageRange, PlatformCapabilities, PlatformError, PlatformHandleRegion,
    PlatformVirtualMemory, RelocationReserve, ScalarBitmapOps, VirtualRange, platform_reserve,
    set_thread_affinity,
};

/// active runtime 切换前的生产类型别名。
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
