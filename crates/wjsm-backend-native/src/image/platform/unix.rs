use std::ffi::c_void;
use std::io;
use std::ptr::NonNull;

use crate::image::ImageLoadError;

pub(crate) struct ExecutableMapping {
    base: NonNull<u8>,
    len: usize,
}

// SAFETY: 映射的可变阶段只存在于 loader 独占构造期；发布后各页权限为 RX/R，且不再写入。
unsafe impl Send for ExecutableMapping {}
// SAFETY: 同上，发布后的映射只读，生命周期由 `CompiledImage` 统一拥有。
unsafe impl Sync for ExecutableMapping {}

impl ExecutableMapping {
    pub(crate) fn allocate(len: usize) -> Result<Self, ImageLoadError> {
        let len = align_to_page(len)?;
        // SAFETY: 参数请求私有匿名 RW 映射；返回值立即检查且由 Drop 对称 munmap。
        let pointer = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if pointer == libc::MAP_FAILED {
            return Err(ImageLoadError::Platform(
                io::Error::last_os_error().to_string(),
            ));
        }
        let base = NonNull::new(pointer.cast::<u8>())
            .ok_or_else(|| ImageLoadError::Platform("mmap returned null".into()))?;
        Ok(Self { base, len })
    }

    pub(crate) fn address(&self) -> usize {
        self.base.as_ptr().addr()
    }

    pub(crate) fn write(&mut self, offset: usize, bytes: &[u8]) -> Result<(), ImageLoadError> {
        let end = offset
            .checked_add(bytes.len())
            .ok_or(ImageLoadError::AddressOverflow)?;
        if end > self.len {
            return Err(ImageLoadError::SectionOutOfBounds);
        }
        // SAFETY: `offset..end` 已验证在独占 RW 映射内，源切片有效且不重叠。
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.base.as_ptr().add(offset),
                bytes.len(),
            );
        }
        Ok(())
    }

    pub(crate) fn read_u32(&self, offset: usize) -> Result<u32, ImageLoadError> {
        let end = offset
            .checked_add(size_of::<u32>())
            .ok_or(ImageLoadError::AddressOverflow)?;
        if end > self.len {
            return Err(ImageLoadError::SectionOutOfBounds);
        }
        // SAFETY: 已验证四字节读取位于映射内；重定位位置不要求自然对齐。
        Ok(unsafe {
            self.base
                .as_ptr()
                .add(offset)
                .cast::<u32>()
                .read_unaligned()
        })
    }

    pub(crate) fn write_u32(&mut self, offset: usize, value: u32) -> Result<(), ImageLoadError> {
        self.write(offset, &value.to_le_bytes())
    }

    pub(crate) fn write_u64(&mut self, offset: usize, value: u64) -> Result<(), ImageLoadError> {
        self.write(offset, &value.to_le_bytes())
    }

    pub(crate) fn finalize_executable(
        &self,
        offset: usize,
        len: usize,
    ) -> Result<(), ImageLoadError> {
        let pointer = self.range_pointer(offset, len)?;
        flush_instruction_cache(pointer, len)?;
        self.protect(offset, len, libc::PROT_READ | libc::PROT_EXEC)
    }

    pub(crate) fn finalize_read_only(
        &self,
        offset: usize,
        len: usize,
    ) -> Result<(), ImageLoadError> {
        self.protect(offset, len, libc::PROT_READ)
    }

    fn protect(&self, offset: usize, len: usize, protection: i32) -> Result<(), ImageLoadError> {
        let pointer = self.range_pointer(offset, len)?;
        // SAFETY: pointer/len 是仍存活 mmap 内的页对齐子区间，权限值由本模块固定选择。
        let result = unsafe { libc::mprotect(pointer.cast::<c_void>(), len, protection) };
        if result == 0 {
            Ok(())
        } else {
            Err(ImageLoadError::Platform(
                io::Error::last_os_error().to_string(),
            ))
        }
    }

    fn range_pointer(&self, offset: usize, len: usize) -> Result<*mut u8, ImageLoadError> {
        let page_size = page_size()?;
        let end = offset
            .checked_add(len)
            .ok_or(ImageLoadError::AddressOverflow)?;
        if !offset.is_multiple_of(page_size)
            || len == 0
            || !len.is_multiple_of(page_size)
            || end > self.len
        {
            return Err(ImageLoadError::SectionOutOfBounds);
        }
        // SAFETY: offset 已验证位于映射内，允许构造该子区间首地址。
        Ok(unsafe { self.base.as_ptr().add(offset) })
    }
}

impl Drop for ExecutableMapping {
    fn drop(&mut self) {
        // SAFETY: base/len 是本对象唯一拥有的完整 mmap 区间，Drop 只执行一次。
        let _ = unsafe { libc::munmap(self.base.as_ptr().cast::<c_void>(), self.len) };
    }
}

pub(crate) fn page_size() -> Result<usize, ImageLoadError> {
    // SAFETY: sysconf 无内存安全前置条件。
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(page_size)
        .ok()
        .filter(|size| *size != 0 && size.is_power_of_two())
        .ok_or_else(|| ImageLoadError::Platform("invalid OS page size".into()))
}

pub(crate) fn align_to_page(len: usize) -> Result<usize, ImageLoadError> {
    len.max(1)
        .checked_next_multiple_of(page_size()?)
        .ok_or(ImageLoadError::AddressOverflow)
}

#[cfg(target_arch = "x86_64")]
fn flush_instruction_cache(_start: *mut u8, _len: usize) -> Result<(), ImageLoadError> {
    Ok(())
}

#[cfg(target_arch = "aarch64")]
fn flush_instruction_cache(start: *mut u8, len: usize) -> Result<(), ImageLoadError> {
    use core::arch::asm;

    const CACHE_LINE_SIZE: usize = 64;
    let start = start.addr();
    let end = start
        .checked_add(len)
        .ok_or(ImageLoadError::AddressOverflow)?;
    let mut line = start & !(CACHE_LINE_SIZE - 1);
    let end = end
        .checked_next_multiple_of(CACHE_LINE_SIZE)
        .ok_or(ImageLoadError::AddressOverflow)?;
    while line < end {
        // SAFETY: ARMv8 允许用户态对当前进程有效地址执行 ic ivau；地址覆盖刚写完的代码页。
        unsafe { asm!("ic ivau, {line}", line = in(reg) line, options(nostack, preserves_flags)) };
        line += CACHE_LINE_SIZE;
    }
    // SAFETY: 屏障只同步当前核心的 cache 与取指流水线，不读写 Rust 内存。
    unsafe {
        asm!("dsb ish", options(nostack, preserves_flags));
        asm!("isb", options(nostack, preserves_flags));
    }
    Ok(())
}
