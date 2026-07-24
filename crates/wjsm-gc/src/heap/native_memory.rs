use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::memory::{GrowableHeapMemory, HeapMemory};
use super::word::{HeapAddress, HeapMemoryError};

/// 原生堆后端：预分配最大容量，经 [`GrowableHeapMemory::grow_to`] 扩展已提交窗口。
///
/// `base` 可设为 `ManagedHeapLayout::object_heap_base()`，使算法用的绝对地址
/// （handle region 之后）映射到 native 缓冲区内相对偏移，避免为 32GiB handle
/// region 分配真实内存。
#[derive(Clone)]
pub struct NativeHeapMemory {
    inner: Arc<NativeHeapInner>,
}

struct NativeHeapInner {
    base: u64,
    /// 已提交字节数（相对 base 的窗口长度）。
    committed: AtomicU64,
    /// 预分配最大容量（相对 base）。
    capacity: u64,
    words: Box<[AtomicU64]>,
}

impl NativeHeapMemory {
    pub fn new(byte_len: u64) -> Self {
        Self::with_base(0, byte_len)
    }

    /// 测试 backend 可用高虚拟 base 模拟 memory64 address，绝不把 address 截断为 u32。
    ///
    /// 预分配 `byte_len` 容量，初始已全部提交；`grow_to` 可在容量内扩展，超出则失败。
    pub fn with_base(base: u64, byte_len: u64) -> Self {
        Self::with_capacity(base, byte_len, byte_len)
    }

    /// 预分配 `capacity` 字节，初始提交 `initial` 字节。
    pub fn with_capacity(base: u64, initial: u64, capacity: u64) -> Self {
        assert!(
            initial <= capacity,
            "native heap initial commit exceeds capacity"
        );
        let word_count = capacity.div_ceil(8);
        let word_count = usize::try_from(word_count).expect("native test heap fits host usize");
        let words = std::iter::repeat_with(|| AtomicU64::new(0))
            .take(word_count)
            .collect();
        Self {
            inner: Arc::new(NativeHeapInner {
                base,
                committed: AtomicU64::new(initial),
                capacity,
                words,
            }),
        }
    }

    /// 为 `ManagedHeapLayout` 构造 object-heap 窗口：base = object_heap_base，
    /// 容量 = object 区大小；初始提交 0，由算法 `grow_to` 推进。
    pub fn for_layout(layout: &super::ManagedHeapLayout) -> Self {
        let base = layout.object_heap_base();
        let capacity = layout.object_heap_end() - layout.object_heap_base();
        Self::with_capacity(base, 0, capacity)
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

    /// 仅用于未发布对象的 raw byte 初始化；CAS 保留同一 word 的其他并发 byte 更新。
    pub fn copy_from(&self, address: HeapAddress, bytes: &[u8]) -> Result<(), HeapMemoryError> {
        <Self as HeapMemory>::copy_from(self, address, bytes)
    }

    pub fn copy_to(&self, address: HeapAddress, length: u64) -> Result<Vec<u8>, HeapMemoryError> {
        <Self as HeapMemory>::copy_to(self, address, length)
    }

    fn committed(&self) -> u64 {
        self.inner.committed.load(Ordering::SeqCst)
    }

    fn checked_offset(&self, address: HeapAddress, length: u64) -> Result<u64, HeapMemoryError> {
        let committed = self.committed();
        let offset =
            address
                .get()
                .checked_sub(self.inner.base)
                .ok_or(HeapMemoryError::OutOfBounds {
                    address: address.get(),
                    length,
                    memory_len: self.inner.base.saturating_add(committed),
                })?;
        let end = offset
            .checked_add(length)
            .ok_or(HeapMemoryError::OutOfBounds {
                address: address.get(),
                length,
                memory_len: self.inner.base.saturating_add(committed),
            })?;
        if end > committed {
            return Err(HeapMemoryError::OutOfBounds {
                address: address.get(),
                length,
                memory_len: self.inner.base.saturating_add(committed),
            });
        }
        Ok(offset)
    }

    fn word(&self, offset: u64) -> &AtomicU64 {
        let index = usize::try_from(offset / 8).expect("checked native heap index fits usize");
        &self.inner.words[index]
    }

    fn write_byte(&self, offset: u64, value: u8) {
        let word = self.word(offset);
        let shift = (offset % 8) * 8;
        let mask = 0xff_u64 << shift;
        let mut previous = word.load(Ordering::SeqCst);
        loop {
            let next = (previous & !mask) | (u64::from(value) << shift);
            match word.compare_exchange(previous, next, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return,
                Err(actual) => previous = actual,
            }
        }
    }
}


impl HeapMemory for NativeHeapMemory {
    fn byte_len(&self) -> u64 {
        // 对算法呈现「从地址 0 起」的绝对已提交上限，与 SharedHeapMemory 语义对齐。
        self.inner.base.saturating_add(self.committed())
    }

    fn load_word(&self, address: HeapAddress) -> Result<u64, HeapMemoryError> {
        if !address.get().is_multiple_of(8) {
            return Err(HeapMemoryError::UnalignedWord {
                address: address.get(),
            });
        }
        let offset = self.checked_offset(address, 8)?;
        Ok(self.word(offset).load(Ordering::SeqCst))
    }

    fn store_word(&self, address: HeapAddress, value: u64) -> Result<(), HeapMemoryError> {
        if !address.get().is_multiple_of(8) {
            return Err(HeapMemoryError::UnalignedWord {
                address: address.get(),
            });
        }
        let offset = self.checked_offset(address, 8)?;
        self.word(offset).store(value, Ordering::SeqCst);
        Ok(())
    }

    fn copy_from(&self, address: HeapAddress, bytes: &[u8]) -> Result<(), HeapMemoryError> {
        let offset = self.checked_offset(
            address,
            u64::try_from(bytes.len()).expect("usize always fits u64"),
        )?;
        for (index, value) in bytes.iter().copied().enumerate() {
            self.write_byte(
                offset + u64::try_from(index).expect("usize always fits u64"),
                value,
            );
        }
        Ok(())
    }

    fn copy_to(&self, address: HeapAddress, length: u64) -> Result<Vec<u8>, HeapMemoryError> {
        let offset = self.checked_offset(address, length)?;
        let length = usize::try_from(length).map_err(|_| HeapMemoryError::AddressTooLarge {
            address: address.get(),
        })?;
        let mut bytes = Vec::with_capacity(length);
        for index in 0..length {
            let byte_offset = offset + u64::try_from(index).expect("usize always fits u64");
            let shift = (byte_offset % 8) * 8;
            bytes.push((self.word(byte_offset).load(Ordering::SeqCst) >> shift) as u8);
        }
        Ok(bytes)
    }
}

impl GrowableHeapMemory for NativeHeapMemory {
    fn maximum_byte_len(&self) -> u64 {
        self.inner.base.saturating_add(self.inner.capacity)
    }

    fn grow_to(&self, byte_len: u64) -> Result<(), String> {
        if byte_len <= self.inner.base {
            return Ok(());
        }
        let needed = byte_len - self.inner.base;
        if needed > self.inner.capacity {
            return Err(format!(
                "native heap grow_to({byte_len}) exceeds capacity {} (base {})",
                self.maximum_byte_len(),
                self.inner.base
            ));
        }
        let mut current = self.inner.committed.load(Ordering::SeqCst);
        while needed > current {
            match self.inner.committed.compare_exchange(
                current,
                needed,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => current = actual,
            }
        }
        Ok(())
    }
}
