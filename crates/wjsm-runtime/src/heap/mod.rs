mod memory;
mod native_memory;
mod word;

pub use memory::SharedHeapMemory;
pub use native_memory::NativeHeapMemory;
pub use word::{HeapAddress, HeapMemoryError};

use memory::HeapMemory;

/// 单态化 managed heap owner；生产路径不会使用 trait object。
// Task 4 只建立类型边界；active Linker 在 Task 15 前不会构造此 owner。
#[allow(dead_code)]
pub(crate) struct ManagedHeap<M> {
    memory: M,
}

#[allow(dead_code)]
impl<M> ManagedHeap<M> {
    pub(crate) fn new(memory: M) -> Self {
        Self { memory }
    }

    pub(crate) fn memory(&self) -> &M {
        &self.memory
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
