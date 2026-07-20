//! macOS virtual memory (mmap / madvise).

use super::PlatformError;

use std::io;
use std::ptr;

pub struct MacOsVirtualMemory;

pub fn reserve(len: usize) -> Result<*mut u8, PlatformError> {
    // SAFETY: anonymous private mapping.
    let ptr = unsafe {
        libc::mmap(
            ptr::null_mut(),
            len,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        Err(PlatformError::Os(io::Error::last_os_error().to_string()))
    } else {
        Ok(ptr.cast())
    }
}

/// SAFETY: `base` is a previously reserved range of at least `len` bytes.
pub unsafe fn commit(base: *mut u8, len: usize) -> Result<(), PlatformError> {
    let rc = unsafe { libc::mprotect(base.cast(), len, libc::PROT_READ | libc::PROT_WRITE) };
    if rc != 0 {
        Err(PlatformError::Os(io::Error::last_os_error().to_string()))
    } else {
        Ok(())
    }
}

/// SAFETY: `base` is a previously reserved range of at least `len` bytes.
pub unsafe fn decommit(base: *mut u8, len: usize) -> Result<(), PlatformError> {
    // madvise FREE is stronger on Darwin; DONTNEED is portable enough here.
    let rc = unsafe { libc::madvise(base.cast(), len, libc::MADV_DONTNEED) };
    if rc != 0 {
        return Err(PlatformError::Os(io::Error::last_os_error().to_string()));
    }
    let rc = unsafe { libc::mprotect(base.cast(), len, libc::PROT_NONE) };
    if rc != 0 {
        Err(PlatformError::Os(io::Error::last_os_error().to_string()))
    } else {
        Ok(())
    }
}

/// SAFETY: `base` is a previously reserved range of `len` bytes.
pub unsafe fn release(base: *mut u8, len: usize) -> Result<(), PlatformError> {
    let rc = unsafe { libc::munmap(base.cast(), len) };
    if rc != 0 {
        Err(PlatformError::Os(io::Error::last_os_error().to_string()))
    } else {
        Ok(())
    }
}
