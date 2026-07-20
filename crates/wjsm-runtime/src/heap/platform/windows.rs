//! Windows virtual memory (VirtualAlloc / VirtualFree).

use super::PlatformError;

pub struct WindowsVirtualMemory;

#[cfg(windows)]
mod imp {
    use super::PlatformError;
    use std::ptr;

    // Minimal FFI to avoid pulling winapi as a hard dep when not on Windows.
    // On Windows hosts these link against kernel32.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn VirtualAlloc(
            lp_address: *mut core::ffi::c_void,
            dw_size: usize,
            fl_allocation_type: u32,
            fl_protect: u32,
        ) -> *mut core::ffi::c_void;
        fn VirtualFree(
            lp_address: *mut core::ffi::c_void,
            dw_size: usize,
            dw_free_type: u32,
        ) -> i32;
    }

    const MEM_COMMIT: u32 = 0x1000;
    const MEM_RESERVE: u32 = 0x2000;
    const MEM_RELEASE: u32 = 0x8000;
    const MEM_DECOMMIT: u32 = 0x4000;
    const PAGE_NOACCESS: u32 = 0x01;
    const PAGE_READWRITE: u32 = 0x04;

    pub fn reserve(len: usize) -> Result<*mut u8, PlatformError> {
        // SAFETY: reserve-only VirtualAlloc.
        let ptr = unsafe {
            VirtualAlloc(ptr::null_mut(), len, MEM_RESERVE, PAGE_NOACCESS)
        };
        if ptr.is_null() {
            Err(PlatformError::OutOfMemory)
        } else {
            Ok(ptr.cast())
        }
    }

    pub unsafe fn commit(base: *mut u8, len: usize) -> Result<(), PlatformError> {
        let ptr = unsafe { VirtualAlloc(base.cast(), len, MEM_COMMIT, PAGE_READWRITE) };
        if ptr.is_null() {
            Err(PlatformError::Os("VirtualAlloc commit failed".into()))
        } else {
            Ok(())
        }
    }

    pub unsafe fn decommit(base: *mut u8, len: usize) -> Result<(), PlatformError> {
        let ok = unsafe { VirtualFree(base.cast(), len, MEM_DECOMMIT) };
        if ok == 0 {
            Err(PlatformError::Os("VirtualFree decommit failed".into()))
        } else {
            Ok(())
        }
    }

    pub unsafe fn release(base: *mut u8, _len: usize) -> Result<(), PlatformError> {
        let ok = unsafe { VirtualFree(base.cast(), 0, MEM_RELEASE) };
        if ok == 0 {
            Err(PlatformError::Os("VirtualFree release failed".into()))
        } else {
            Ok(())
        }
    }

    pub fn has_hard_isolation() -> bool {
        // Job Object probing is deferred to gc-bench resource_platform; report soft.
        false
    }
}

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
pub fn reserve(_len: usize) -> Result<*mut u8, PlatformError> {
    Err(PlatformError::Unsupported("windows backend"))
}

#[cfg(not(windows))]
pub unsafe fn commit(_base: *mut u8, _len: usize) -> Result<(), PlatformError> {
    Err(PlatformError::Unsupported("windows backend"))
}

#[cfg(not(windows))]
pub unsafe fn decommit(_base: *mut u8, _len: usize) -> Result<(), PlatformError> {
    Err(PlatformError::Unsupported("windows backend"))
}

#[cfg(not(windows))]
pub unsafe fn release(_base: *mut u8, _len: usize) -> Result<(), PlatformError> {
    Err(PlatformError::Unsupported("windows backend"))
}

#[cfg(not(windows))]
pub fn has_hard_isolation() -> bool {
    false
}
