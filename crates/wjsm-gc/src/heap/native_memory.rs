use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::memory::{GrowableHeapMemory, HeapMemory};
use super::platform::{PlatformError, VirtualRange, reserve};
use super::word::{HeapAddress, HeapMemoryError};

const COMMIT_GRANULE_BYTES: u64 = 64 * 1024;

/// 生产 native 堆：保留完整逻辑容量，并按需提交 OS 虚拟内存页。
///
/// JS/GC 使用 `base` 起始的 memory64 逻辑地址；宿主指针只在本实现内部短暂解析，
/// 不会进入 handle、side table 或 snapshot。提交窗口单调增长，因此发布后的地址稳定。
#[derive(Clone)]
pub struct NativeHeapMemory {
    inner: Arc<NativeHeapInner>,
}

struct NativeHeapInner {
    logical_base: u64,
    committed: AtomicU64,
    capacity: u64,
    virtual_base: *mut u8,
    range: Mutex<VirtualRange>,
}

// SAFETY: `VirtualRange` 独占 reservation；读写只进入已提交窗口，word 访问使用原子操作。
unsafe impl Send for NativeHeapInner {}
// SAFETY: 扩展提交窗口由 `range` 串行化，`committed` 以 release/acquire 发布。
unsafe impl Sync for NativeHeapInner {}

impl NativeHeapMemory {
    /// 为 `ManagedHeapLayout` 保留 object-heap address space，初始不提交物理页。
    pub fn for_layout(layout: &super::ManagedHeapLayout) -> Result<Self, PlatformError> {
        let logical_base = layout.object_heap_base();
        let capacity = layout.object_heap_end() - logical_base;
        Self::with_capacity(logical_base, capacity)
    }

    pub fn with_capacity(logical_base: u64, capacity: u64) -> Result<Self, PlatformError> {
        let capacity = align_commit(capacity).ok_or(PlatformError::InvalidRange)?;
        let capacity = usize::try_from(capacity).map_err(|_| PlatformError::InvalidRange)?;
        let range = reserve(capacity)?;
        let virtual_base = range.base();
        Ok(Self {
            inner: Arc::new(NativeHeapInner {
                logical_base,
                committed: AtomicU64::new(0),
                capacity: capacity as u64,
                virtual_base,
                range: Mutex::new(range),
            }),
        })
    }

    fn committed(&self) -> u64 {
        self.inner.committed.load(Ordering::Acquire)
    }

    fn checked_offset(&self, address: HeapAddress, length: u64) -> Result<u64, HeapMemoryError> {
        let committed = self.committed();
        let offset = address.get().checked_sub(self.inner.logical_base).ok_or(
            HeapMemoryError::OutOfBounds {
                address: address.get(),
                length,
                memory_len: self.inner.logical_base.saturating_add(committed),
            },
        )?;
        let end = offset
            .checked_add(length)
            .ok_or(HeapMemoryError::OutOfBounds {
                address: address.get(),
                length,
                memory_len: self.inner.logical_base.saturating_add(committed),
            })?;
        if end > committed {
            return Err(HeapMemoryError::OutOfBounds {
                address: address.get(),
                length,
                memory_len: self.inner.logical_base.saturating_add(committed),
            });
        }
        Ok(offset)
    }

    fn word(&self, offset: u64) -> Result<&AtomicU64, HeapMemoryError> {
        let offset = usize::try_from(offset).map_err(|_| HeapMemoryError::AddressTooLarge {
            address: self.inner.logical_base.saturating_add(offset),
        })?;
        // SAFETY: reservation base 以页对齐；调用方已验证 offset 对齐且落在已提交窗口。
        Ok(unsafe { &*self.inner.virtual_base.add(offset).cast::<AtomicU64>() })
    }

    fn write_byte(&self, offset: u64, value: u8) -> Result<(), HeapMemoryError> {
        let word_offset = offset & !7;
        let word = self.word(word_offset)?;
        let shift = (offset % 8) * 8;
        let mask = 0xff_u64 << shift;
        let mut previous = word.load(Ordering::SeqCst);
        loop {
            let next = (previous & !mask) | (u64::from(value) << shift);
            match word.compare_exchange(previous, next, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Ok(()),
                Err(actual) => previous = actual,
            }
        }
    }
}

impl HeapMemory for NativeHeapMemory {
    fn byte_len(&self) -> u64 {
        self.inner.logical_base.saturating_add(self.committed())
    }

    fn load_word(&self, address: HeapAddress) -> Result<u64, HeapMemoryError> {
        if !address.get().is_multiple_of(8) {
            return Err(HeapMemoryError::UnalignedWord {
                address: address.get(),
            });
        }
        let offset = self.checked_offset(address, 8)?;
        Ok(self.word(offset)?.load(Ordering::SeqCst))
    }

    fn store_word(&self, address: HeapAddress, value: u64) -> Result<(), HeapMemoryError> {
        if !address.get().is_multiple_of(8) {
            return Err(HeapMemoryError::UnalignedWord {
                address: address.get(),
            });
        }
        let offset = self.checked_offset(address, 8)?;
        self.word(offset)?.store(value, Ordering::SeqCst);
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
            )?;
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
            bytes.push((self.word(byte_offset & !7)?.load(Ordering::SeqCst) >> shift) as u8);
        }
        Ok(bytes)
    }
    fn try_bytes(&self, address: HeapAddress, length: u64) -> Option<&[u8]> {
        let offset = usize::try_from(self.checked_offset(address, length).ok()?).ok()?;
        let length = usize::try_from(length).ok()?;
        // SAFETY: checked_offset 证明区间落在已提交窗口内；reservation 的虚拟基址
        // 在整个 NativeHeapMemory 生命周期内稳定，调用方只在无搬迁读取作用域内借用。
        Some(unsafe { std::slice::from_raw_parts(self.inner.virtual_base.add(offset), length) })
    }

    fn copy_nonoverlapping_unpublished(
        &self,
        source: HeapAddress,
        destination: HeapAddress,
        length: u64,
    ) -> Result<(), HeapMemoryError> {
        let source_offset = self.checked_offset(source, length)?;
        let destination_offset = self.checked_offset(destination, length)?;
        let source_end = source_offset + length;
        let destination_end = destination_offset + length;
        if source_offset < destination_end && destination_offset < source_end {
            return Err(HeapMemoryError::OverlappingCopy {
                source: source.get(),
                destination: destination.get(),
                length,
            });
        }
        let source_offset =
            usize::try_from(source_offset).map_err(|_| HeapMemoryError::AddressTooLarge {
                address: source.get(),
            })?;
        let destination_offset =
            usize::try_from(destination_offset).map_err(|_| HeapMemoryError::AddressTooLarge {
                address: destination.get(),
            })?;
        let length = usize::try_from(length).map_err(|_| HeapMemoryError::AddressTooLarge {
            address: source.get(),
        })?;
        // SAFETY: checked_offset 证明两段都位于同一 live reservation 内；上面的区间
        // 检查证明不重叠，且本 API 只允许用于未 publish、无并发访问的范围。
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.inner.virtual_base.add(source_offset),
                self.inner.virtual_base.add(destination_offset),
                length,
            );
        }
        Ok(())
    }

    fn copy_atomic_words(
        &self,
        source: HeapAddress,
        destination: HeapAddress,
        length: u64,
    ) -> Result<(), HeapMemoryError> {
        if !length.is_multiple_of(8) {
            return Err(HeapMemoryError::InvalidAtomicCopyLength { length });
        }
        let mut offset = 0;
        while offset < length {
            let word = self.load_word(HeapAddress::new(source.get() + offset))?;
            self.store_word(HeapAddress::new(destination.get() + offset), word)?;
            offset += 8;
        }
        Ok(())
    }
}

impl GrowableHeapMemory for NativeHeapMemory {
    fn logical_base(&self) -> u64 {
        self.inner.logical_base
    }

    fn virtual_base(&self) -> *mut u8 {
        self.inner.virtual_base
    }

    fn maximum_byte_len(&self) -> u64 {
        self.inner.logical_base.saturating_add(self.inner.capacity)
    }

    fn grow_to(&self, byte_len: u64) -> Result<(), String> {
        if byte_len <= self.inner.logical_base {
            return Ok(());
        }
        let needed = byte_len - self.inner.logical_base;
        let committed = align_commit(needed)
            .ok_or_else(|| format!("native heap grow_to({byte_len}) overflows commit granule"))?;
        if committed > self.inner.capacity {
            return Err(format!(
                "native heap grow_to({byte_len}) exceeds capacity {} (base {})",
                self.maximum_byte_len(),
                self.inner.logical_base
            ));
        }
        if committed <= self.committed() {
            return Ok(());
        }
        let mut range = self
            .inner
            .range
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current = self.committed();
        if committed <= current {
            return Ok(());
        }
        range
            .commit(
                usize::try_from(current).map_err(|error| error.to_string())?,
                usize::try_from(committed - current).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        self.inner.committed.store(committed, Ordering::Release);
        Ok(())
    }
}

/// GC 协议测试专用的有界原子 word 缓冲区，不得进入 production manifest。
#[derive(Clone)]
pub struct TestHeapMemory {
    inner: Arc<TestHeapInner>,
}

struct TestHeapInner {
    base: u64,
    committed: AtomicU64,
    capacity: u64,
    words: Box<[AtomicU64]>,
}

impl TestHeapMemory {
    pub fn new(byte_len: u64) -> Self {
        Self::with_base(0, byte_len)
    }

    pub fn with_base(base: u64, byte_len: u64) -> Self {
        Self::with_capacity(base, byte_len, byte_len)
    }

    pub fn with_capacity(base: u64, initial: u64, capacity: u64) -> Self {
        assert!(
            initial <= capacity,
            "test heap initial commit exceeds capacity"
        );
        let word_count = usize::try_from(capacity.div_ceil(8)).expect("test heap fits host usize");
        let words = std::iter::repeat_with(|| AtomicU64::new(0))
            .take(word_count)
            .collect();
        Self {
            inner: Arc::new(TestHeapInner {
                base,
                committed: AtomicU64::new(initial),
                capacity,
                words,
            }),
        }
    }

    pub fn for_layout(layout: &super::ManagedHeapLayout) -> Self {
        let base = layout.object_heap_base();
        Self::with_capacity(base, 0, layout.object_heap_end() - base)
    }

    pub fn load_word(&self, address: HeapAddress) -> Result<u64, HeapMemoryError> {
        <Self as HeapMemory>::load_word(self, address)
    }

    pub fn store_word(&self, address: HeapAddress, value: u64) -> Result<(), HeapMemoryError> {
        <Self as HeapMemory>::store_word(self, address, value)
    }

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
        &self.inner.words[usize::try_from(offset / 8).expect("checked test heap index fits usize")]
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

impl HeapMemory for TestHeapMemory {
    fn byte_len(&self) -> u64 {
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

    fn copy_nonoverlapping_unpublished(
        &self,
        source: HeapAddress,
        destination: HeapAddress,
        length: u64,
    ) -> Result<(), HeapMemoryError> {
        let source_offset = self.checked_offset(source, length)?;
        let destination_offset = self.checked_offset(destination, length)?;
        let source_end = source_offset + length;
        let destination_end = destination_offset + length;
        if source_offset < destination_end && destination_offset < source_end {
            return Err(HeapMemoryError::OverlappingCopy {
                source: source.get(),
                destination: destination.get(),
                length,
            });
        }
        for offset in 0..length {
            let source_byte = source_offset + offset;
            let shift = (source_byte % 8) * 8;
            let value = (self.word(source_byte).load(Ordering::SeqCst) >> shift) as u8;
            self.write_byte(destination_offset + offset, value);
        }
        Ok(())
    }

    fn copy_atomic_words(
        &self,
        source: HeapAddress,
        destination: HeapAddress,
        length: u64,
    ) -> Result<(), HeapMemoryError> {
        if !length.is_multiple_of(8) {
            return Err(HeapMemoryError::InvalidAtomicCopyLength { length });
        }
        let mut offset = 0;
        while offset < length {
            let word = self.load_word(HeapAddress::new(source.get() + offset))?;
            self.store_word(HeapAddress::new(destination.get() + offset), word)?;
            offset += 8;
        }
        Ok(())
    }
}

impl GrowableHeapMemory for TestHeapMemory {
    fn logical_base(&self) -> u64 {
        self.inner.base
    }
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
                "test heap grow_to({byte_len}) exceeds capacity {} (base {})",
                self.maximum_byte_len(),
                self.inner.base
            ));
        }
        self.inner.committed.fetch_max(needed, Ordering::SeqCst);
        Ok(())
    }
}

fn align_commit(value: u64) -> Option<u64> {
    let remainder = value % COMMIT_GRANULE_BYTES;
    if remainder == 0 {
        Some(value)
    } else {
        value.checked_add(COMMIT_GRANULE_BYTES - remainder)
    }
}
