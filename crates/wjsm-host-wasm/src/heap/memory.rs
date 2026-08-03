use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use wasmtime::SharedMemory;
use wjsm_gc::heap::{GrowableHeapMemory, HeapAddress, HeapMemory, HeapMemoryError};

/// 缓存的共享内存基址/长度。
///
/// wasmtime 保证 SharedMemory 基址在生命周期内稳定（grow 只改长度且单调增长），
/// 热路径 word 访问（闭包 env 槽位读写等）因此无需每次调用 `SharedMemory::data()`
/// （profile 自时间 ~11%）；命中缓存长度即一次原子读。越界时重新取 `data()` 刷新
/// 缓存后复检，以区分真实的越界错误。
#[derive(Debug)]
struct CachedRange {
    base: AtomicPtr<u8>,
    len: AtomicUsize,
}

/// 对 Wasmtime shared memory64 的 Store-free 包装。
#[derive(Clone)]
pub struct SharedHeapMemory {
    memory: SharedMemory,
    cached: Arc<CachedRange>,
}

impl SharedHeapMemory {
    pub fn new(memory: SharedMemory) -> Self {
        let data = memory.data();
        let cached = Arc::new(CachedRange {
            base: AtomicPtr::new(data.as_ptr().cast::<u8>().cast_mut()),
            len: AtomicUsize::new(data.len()),
        });
        Self { memory, cached }
    }

    pub fn byte_len(&self) -> u64 {
        <Self as HeapMemory>::byte_len(self)
    }

    pub fn maximum_byte_len(&self) -> u64 {
        <Self as GrowableHeapMemory>::maximum_byte_len(self)
    }

    pub fn grow_to(&self, byte_len: u64) -> Result<(), String> {
        <Self as GrowableHeapMemory>::grow_to(self, byte_len)
    }

    pub fn load_word(&self, address: HeapAddress) -> Result<u64, HeapMemoryError> {
        <Self as HeapMemory>::load_word(self, address)
    }

    pub fn store_word(&self, address: HeapAddress, value: u64) -> Result<(), HeapMemoryError> {
        <Self as HeapMemory>::store_word(self, address, value)
    }

    /// 仅用于尚未发布对象的 raw byte 初始化；不得与同一 word 的并发原子访问交叠。
    pub fn copy_from(&self, address: HeapAddress, bytes: &[u8]) -> Result<(), HeapMemoryError> {
        <Self as HeapMemory>::copy_from(self, address, bytes)
    }

    /// 返回 raw byte 的 owned snapshot，避免把 shared `UnsafeCell` 借用泄漏到调用方。
    pub fn copy_to(&self, address: HeapAddress, length: u64) -> Result<Vec<u8>, HeapMemoryError> {
        <Self as HeapMemory>::copy_to(self, address, length)
    }

    /// 从 shared memory64 读取 nul-terminated UTF-8 byte 序列的 owned snapshot。
    pub fn read_c_string(&self, address: HeapAddress) -> Result<Vec<u8>, HeapMemoryError> {
        const CHUNK_BYTES: u64 = 64 * 1024;

        let mut current = address.get();
        let end = self.byte_len();
        let mut bytes = Vec::new();
        while current < end {
            let length = (end - current).min(CHUNK_BYTES);
            let chunk = self.copy_to(HeapAddress::new(current), length)?;
            if let Some(nul) = chunk.iter().position(|byte| *byte == 0) {
                bytes.extend_from_slice(&chunk[..nul]);
                return Ok(bytes);
            }
            bytes.extend_from_slice(&chunk);
            current += length;
        }
        Ok(bytes)
    }

    fn checked_index(&self, address: HeapAddress, length: u64) -> Result<usize, HeapMemoryError> {
        let memory_len = u64::try_from(self.memory.data().len()).expect("usize always fits u64");
        let end = address
            .get()
            .checked_add(length)
            .ok_or(HeapMemoryError::OutOfBounds {
                address: address.get(),
                length,
                memory_len,
            })?;
        if end > memory_len {
            return Err(HeapMemoryError::OutOfBounds {
                address: address.get(),
                length,
                memory_len,
            });
        }
        usize::try_from(address.get()).map_err(|_| HeapMemoryError::AddressTooLarge {
            address: address.get(),
        })
    }

    /// 刷新缓存的基址/长度（grow 后长度单调增长；基址不变）。
    fn refresh_cached(&self) {
        let data = self.memory.data();
        self.cached
            .base
            .store(data.as_ptr().cast::<u8>().cast_mut(), Ordering::Relaxed);
        self.cached.len.store(data.len(), Ordering::Relaxed);
    }

    /// 快速索引检查：命中缓存长度即返回；越界时刷新后复检（区分真实越界）。
    fn checked_index_cached(
        &self,
        address: HeapAddress,
        length: u64,
    ) -> Result<usize, HeapMemoryError> {
        let end = address
            .get()
            .checked_add(length)
            .ok_or(HeapMemoryError::OutOfBounds {
                address: address.get(),
                length,
                memory_len: self.cached.len.load(Ordering::Relaxed) as u64,
            })?;
        if end > self.cached.len.load(Ordering::Relaxed) as u64 {
            self.refresh_cached();
            let memory_len = self.cached.len.load(Ordering::Relaxed) as u64;
            if end > memory_len {
                return Err(HeapMemoryError::OutOfBounds {
                    address: address.get(),
                    length,
                    memory_len,
                });
            }
        }
        usize::try_from(address.get()).map_err(|_| HeapMemoryError::AddressTooLarge {
            address: address.get(),
        })
    }

    fn word_ptr(&self, address: HeapAddress) -> Result<*mut u64, HeapMemoryError> {
        if !address.get().is_multiple_of(8) {
            return Err(HeapMemoryError::UnalignedWord {
                address: address.get(),
            });
        }
        let index = self.checked_index_cached(address, 8)?;
        let base = self.cached.base.load(Ordering::Relaxed);
        // SAFETY: `checked_index_cached` 证明整个 u64 位于当前共享内存范围内
        // （越界时已刷新缓存并复检）；调用方已检查 8 字节对齐。wasmtime 保证
        // SharedMemory 基址在生命周期内稳定。后续并发 word 访问一律用 AtomicU64。
        Ok(unsafe { base.add(index).cast::<u64>() })
    }
}

impl HeapMemory for SharedHeapMemory {
    fn byte_len(&self) -> u64 {
        u64::try_from(self.memory.data().len()).expect("usize always fits u64")
    }

    fn load_word(&self, address: HeapAddress) -> Result<u64, HeapMemoryError> {
        let word = self.word_ptr(address)?;
        // SAFETY: `word_ptr` establishes range/alignment/stable mapping; SharedMemory requires
        // Atomic access for concurrent bytes and this is a SeqCst shared value/header word load.
        Ok(unsafe { AtomicU64::from_ptr(word).load(Ordering::SeqCst) })
    }

    fn store_word(&self, address: HeapAddress, value: u64) -> Result<(), HeapMemoryError> {
        let word = self.word_ptr(address)?;
        // SAFETY: see `load_word`; storing through AtomicU64 preserves the Wasm shared-memory
        // atomic contract for all value and mutable-header words.
        unsafe { AtomicU64::from_ptr(word).store(value, Ordering::SeqCst) };
        Ok(())
    }

    fn copy_from(&self, address: HeapAddress, bytes: &[u8]) -> Result<(), HeapMemoryError> {
        let index = self.checked_index(
            address,
            u64::try_from(bytes.len()).expect("usize always fits u64"),
        )?;
        let cells = self.memory.data();
        for (offset, value) in bytes.iter().copied().enumerate() {
            // SAFETY: checked range covers index + offset; AtomicU8 is required by Wasmtime for
            // raw shared bytes and this API is restricted to unpublished, non-overlapping ranges.
            unsafe {
                AtomicU8::from_ptr(cells.as_ptr().cast::<u8>().add(index + offset).cast_mut())
                    .store(value, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    fn copy_to(&self, address: HeapAddress, length: u64) -> Result<Vec<u8>, HeapMemoryError> {
        let index = self.checked_index(address, length)?;
        let length = usize::try_from(length).map_err(|_| HeapMemoryError::AddressTooLarge {
            address: address.get(),
        })?;
        let cells: &[UnsafeCell<u8>] = self.memory.data();
        let mut bytes = Vec::with_capacity(length);
        for offset in 0..length {
            // SAFETY: checked range covers index + offset; AtomicU8 avoids non-atomic reads from
            // Wasmtime shared memory while producing an owned byte snapshot.
            bytes.push(unsafe {
                AtomicU8::from_ptr(cells.as_ptr().cast::<u8>().add(index + offset).cast_mut())
                    .load(Ordering::SeqCst)
            });
        }
        Ok(bytes)
    }

    /// 覆写默认实现：复用 SharedHeapMemory 的 memchr 快路径。
    fn read_c_string(&self, address: HeapAddress) -> Result<Vec<u8>, HeapMemoryError> {
        SharedHeapMemory::read_c_string(self, address)
    }
}

impl GrowableHeapMemory for SharedHeapMemory {
    fn maximum_byte_len(&self) -> u64 {
        self.memory
            .ty()
            .maximum()
            .expect("V2 shared heap requires a finite maximum")
            * 64
            * 1024
    }

    fn grow_to(&self, byte_len: u64) -> Result<(), String> {
        let current = self.byte_len();
        if byte_len <= current {
            return Ok(());
        }
        let target_pages = byte_len.div_ceil(64 * 1024);
        let current_pages = current / (64 * 1024);
        self.memory
            .grow(target_pages - current_pages)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
