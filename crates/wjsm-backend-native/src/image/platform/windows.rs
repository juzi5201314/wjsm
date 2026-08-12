use std::io;
use std::ptr::NonNull;

use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READONLY, PAGE_READWRITE,
    VirtualAlloc, VirtualFree, VirtualProtect,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

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
        // SAFETY: 请求独占的 committed/reserved RW 区域；返回值立即检查并由 Drop 释放。
        let pointer = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                len,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        let base = NonNull::new(pointer.cast::<u8>()).ok_or_else(last_platform_error)?;
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
        let pointer = self.protect(offset, len, PAGE_EXECUTE_READ)?;
        // SAFETY: pointer/len 是刚完成写入并已发布为 RX 的当前进程映射区间。
        let flushed =
            unsafe { FlushInstructionCache(GetCurrentProcess(), pointer.cast_const().cast(), len) };
        if flushed == 0 {
            Err(last_platform_error())
        } else {
            Ok(())
        }
    }

    pub(crate) fn finalize_read_only(
        &self,
        offset: usize,
        len: usize,
    ) -> Result<(), ImageLoadError> {
        self.protect(offset, len, PAGE_READONLY).map(|_| ())
    }

    fn protect(
        &self,
        offset: usize,
        len: usize,
        protection: u32,
    ) -> Result<*mut u8, ImageLoadError> {
        let pointer = self.range_pointer(offset, len)?;
        let mut previous = 0;
        // SAFETY: pointer/len 是仍存活 VirtualAlloc 区域内的页对齐子区间；权限固定为 R/RX。
        let protected =
            unsafe { VirtualProtect(pointer.cast(), len, protection, &raw mut previous) };
        if protected == 0 {
            Err(last_platform_error())
        } else {
            Ok(pointer)
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
        // SAFETY: base 是本对象唯一拥有的完整 VirtualAlloc 区域，MEM_RELEASE 要求 size=0。
        let _ = unsafe { VirtualFree(self.base.as_ptr().cast(), 0, MEM_RELEASE) };
    }
}

pub(crate) fn page_size() -> Result<usize, ImageLoadError> {
    let mut info = SYSTEM_INFO::default();
    // SAFETY: `info` 是有效的可写 SYSTEM_INFO，GetSystemInfo 无失败返回路径。
    unsafe { GetSystemInfo(&raw mut info) };
    usize::try_from(info.dwPageSize)
        .ok()
        .filter(|size| *size != 0 && size.is_power_of_two())
        .ok_or_else(|| ImageLoadError::Platform("invalid OS page size".into()))
}

pub(crate) fn align_to_page(len: usize) -> Result<usize, ImageLoadError> {
    len.max(1)
        .checked_next_multiple_of(page_size()?)
        .ok_or(ImageLoadError::AddressOverflow)
}

fn last_platform_error() -> ImageLoadError {
    ImageLoadError::Platform(io::Error::last_os_error().to_string())
}
