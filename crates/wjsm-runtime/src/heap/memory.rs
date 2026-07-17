use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use wasmtime::SharedMemory;

use super::word::{HeapAddress, HeapMemoryError};

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// 共享 heap 的 checked word/range 接口。生产路径通过泛型单态化，禁止 `dyn HeapMemory`。
pub(crate) trait HeapMemory: sealed::Sealed + Send + Sync {
    fn byte_len(&self) -> u64;
    fn load_word(&self, address: HeapAddress) -> Result<u64, HeapMemoryError>;
    fn store_word(&self, address: HeapAddress, value: u64) -> Result<(), HeapMemoryError>;
    fn copy_from(&self, address: HeapAddress, bytes: &[u8]) -> Result<(), HeapMemoryError>;
    fn copy_to(&self, address: HeapAddress, length: u64) -> Result<Vec<u8>, HeapMemoryError>;
}

/// 对 Wasmtime shared memory64 的 Store-free 包装。
#[derive(Clone)]
pub struct SharedHeapMemory {
    memory: SharedMemory,
}

impl SharedHeapMemory {
    pub fn new(memory: SharedMemory) -> Self {
        Self { memory }
    }

    pub fn byte_len(&self) -> u64 {
        <Self as HeapMemory>::byte_len(self)
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

    fn word_ptr(&self, address: HeapAddress) -> Result<*mut u64, HeapMemoryError> {
        if address.get() % 8 != 0 {
            return Err(HeapMemoryError::UnalignedWord {
                address: address.get(),
            });
        }
        let index = self.checked_index(address, 8)?;
        let bytes = self.memory.data();
        // SAFETY: `checked_index` proves the whole u64 lies within the current shared-memory
        // slice; the caller checked 8-byte alignment. Wasmtime guarantees a stable base pointer
        // for the lifetime of SharedMemory. All concurrent word access below uses AtomicU64.
        Ok(unsafe {
            bytes
                .as_ptr()
                .cast::<u8>()
                .add(index)
                .cast_mut()
                .cast::<u64>()
        })
    }
}

impl sealed::Sealed for SharedHeapMemory {}

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
}
