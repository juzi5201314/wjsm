//! 后端无关的堆内存抽象。
//!
//! [`HeapMemory`] 是 GC 算法与后端堆之间的接缝：算法只经此接口读写
//! word/range，不关心底层是 wasmtime shared memory64（wasm 后端）还是
//! 原生堆（native 后端，见 [`super::native_memory::NativeHeapMemory`]）。
//!
//! 生产路径通过泛型单态化（`M: HeapMemory`），禁止 `dyn HeapMemory`。

use super::word::{HeapAddress, HeapMemoryError};

/// 后端实现 `HeapMemory` 的接缝：host-wasm 的 `SharedHeapMemory` 与本 crate
/// 的 `NativeHeapMemory` 都在此实现。算法经泛型 `M: HeapMemory` 单态化。
pub trait HeapMemory: Send + Sync {
    fn byte_len(&self) -> u64;
    fn load_word(&self, address: HeapAddress) -> Result<u64, HeapMemoryError>;
    fn store_word(&self, address: HeapAddress, value: u64) -> Result<(), HeapMemoryError>;
    fn copy_from(&self, address: HeapAddress, bytes: &[u8]) -> Result<(), HeapMemoryError>;
    fn copy_to(&self, address: HeapAddress, length: u64) -> Result<Vec<u8>, HeapMemoryError>;
}

/// 可增长堆的后端无关接口。
///
/// 从 `SharedHeapMemory`（wasmtime）原有的 `maximum_byte_len`/`grow_to` 提炼，
/// 让 GC 算法（MarkSweepV2/G1V2/ZgcV2）能泛型化于可增长堆，而不绑定 wasmtime。
pub trait GrowableHeapMemory: HeapMemory {
    /// 堆可增长到的最大字节数（含保留未提交部分）。
    fn maximum_byte_len(&self) -> u64;
    /// 把堆已提交区域增长到 `byte_len`。
    fn grow_to(&self, byte_len: u64) -> Result<(), String>;
}
