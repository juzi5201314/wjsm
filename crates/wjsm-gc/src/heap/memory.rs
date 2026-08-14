//! 后端无关的堆内存抽象。
//!
//! [`HeapMemory`] 是 GC 算法与 native 后端堆之间的接缝：算法只经此接口读写
//! word/range，不关心底层虚拟内存实现。
//!
//! 生产路径通过泛型单态化（`M: HeapMemory`），禁止 `dyn HeapMemory`。

use super::word::{HeapAddress, HeapMemoryError};

/// native production heap 与测试 heap 都经此接口实现；算法经泛型单态化。
///
pub trait HeapMemory: Send + Sync {
    fn byte_len(&self) -> u64;
    fn load_word(&self, address: HeapAddress) -> Result<u64, HeapMemoryError>;
    fn store_word(&self, address: HeapAddress, value: u64) -> Result<(), HeapMemoryError>;
    fn copy_from(&self, address: HeapAddress, bytes: &[u8]) -> Result<(), HeapMemoryError>;
    fn copy_to(&self, address: HeapAddress, length: u64) -> Result<Vec<u8>, HeapMemoryError>;

    /// 读取 nul 结尾字节串的 owned snapshot。
    ///
    /// 默认实现按 `copy_to` 分块扫描；具体后端可用直接内存视图覆写以加速。
    fn read_c_string(&self, address: HeapAddress) -> Result<Vec<u8>, HeapMemoryError> {
        const CHUNK: u64 = 4096;
        let base = address.get();
        let mut buf = Vec::new();
        let mut offset = 0u64;
        loop {
            let chunk = self.copy_to(HeapAddress::new(base + offset), CHUNK.min(1024))?;
            if let Some(pos) = chunk.iter().position(|&b| b == 0) {
                buf.extend_from_slice(&chunk[..pos]);
                return Ok(buf);
            }
            buf.extend_from_slice(&chunk);
            if chunk.len() < CHUNK as usize {
                return Ok(buf);
            }
            offset += CHUNK;
        }
    }
}

/// 可增长堆的后端无关接口。
///
/// 从可增长 native heap 的 `maximum_byte_len`/`grow_to` 能力提炼，
/// 让 GC 算法保持后端无关。
pub trait GrowableHeapMemory: HeapMemory {
    /// 该 backend 对外呈现的 memory64 逻辑起点。
    fn logical_base(&self) -> u64;

    /// 堆可增长到的最大字节数（含保留未提交部分）。
    fn maximum_byte_len(&self) -> u64;
    /// 把堆已提交区域增长到 `byte_len`。
    fn grow_to(&self, byte_len: u64) -> Result<(), String>;

    /// 逻辑地址对应的真实虚拟基址。默认实现视逻辑地址即虚拟地址
    /// （TestHeapMemory 语义）；真实平台后端（如 `NativeHeapMemory`）覆盖。
    fn virtual_base(&self) -> *mut u8 {
        self.logical_base() as *mut u8
    }
}
