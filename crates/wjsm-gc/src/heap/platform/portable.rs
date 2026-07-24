//! Portable virtual-memory and scalar bitmap ops.
//!
//! Always available. Platforms without native decommit report the limitation
//! via [`super::PlatformCapabilities`].

use super::PlatformError;

/// Anonymous process-heap backed reservation (no OS decommit).
#[allow(dead_code)]
pub struct PortableVirtualMemory;

pub fn page_size() -> usize {
    4096
}

#[allow(dead_code)]
pub fn reserve(len: usize) -> Result<*mut u8, PlatformError> {
    let layout = layout_for(len)?;
    // SAFETY: layout size is non-zero and aligned.
    let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
    if ptr.is_null() {
        Err(PlatformError::OutOfMemory)
    } else {
        Ok(ptr)
    }
}

/// SAFETY: `base` must come from [`reserve`] with matching `len`.
pub unsafe fn commit(_base: *mut u8, _len: usize) -> Result<(), PlatformError> {
    // Portable path is already committed (heap allocation).
    Ok(())
}

/// SAFETY: `base` must come from [`reserve`] with matching range.
pub unsafe fn decommit(base: *mut u8, len: usize) -> Result<(), PlatformError> {
    if base.is_null() || len == 0 {
        return Err(PlatformError::InvalidRange);
    }
    // Best-effort zero; cannot return pages to the OS on this backend.
    // SAFETY: caller guarantees the range is owned.
    unsafe {
        std::ptr::write_bytes(base, 0, len);
    }
    Ok(())
}

/// SAFETY: `base` must come from [`reserve`] with matching `len`.
pub unsafe fn release(base: *mut u8, len: usize) -> Result<(), PlatformError> {
    if base.is_null() {
        return Ok(());
    }
    let layout = layout_for(len)?;
    // SAFETY: allocation came from `alloc_zeroed` with this layout.
    unsafe {
        std::alloc::dealloc(base, layout);
    }
    Ok(())
}

fn layout_for(len: usize) -> Result<std::alloc::Layout, PlatformError> {
    std::alloc::Layout::from_size_align(len, page_size()).map_err(|_| PlatformError::InvalidRange)
}

/// Scalar bitmap primitives shared with every ISA path for parity tests.
pub struct ScalarBitmapOps;

impl ScalarBitmapOps {
    pub fn count_ones(words: &[u64]) -> u64 {
        words.iter().map(|w| w.count_ones() as u64).sum()
    }

    pub fn or_assign(dst: &mut [u64], src: &[u64]) {
        assert_eq!(dst.len(), src.len());
        for (d, s) in dst.iter_mut().zip(src.iter()) {
            *d |= *s;
        }
    }

    pub fn clear(words: &mut [u64]) {
        for w in words {
            *w = 0;
        }
    }
}
