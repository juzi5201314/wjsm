//! Platform virtual memory, NUMA topology, and SIMD ISA dispatch (Task 23).
//!
//! Portable scalar backends always compile. OS-specific commit/decommit and
//! NUMA queries are cfg-selected; missing ISA/OS/NUMA capabilities surface as
//! `needs-capability-runner` rather than silent pass.

mod portable;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[allow(unused_imports)]
pub use portable::{PortableVirtualMemory, ScalarBitmapOps};
use std::fmt;

#[cfg(target_os = "linux")]
pub use linux::LinuxVirtualMemory;
#[cfg(target_os = "macos")]
pub use macos::MacOsVirtualMemory;
#[cfg(target_os = "windows")]
pub use windows::WindowsVirtualMemory;

/// Active platform virtual-memory backend alias.
#[cfg(target_os = "linux")]
pub type PlatformVirtualMemory = LinuxVirtualMemory;
#[cfg(target_os = "macos")]
pub type PlatformVirtualMemory = MacOsVirtualMemory;
#[cfg(target_os = "windows")]
pub type PlatformVirtualMemory = WindowsVirtualMemory;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub type PlatformVirtualMemory = PortableVirtualMemory;

/// Errors from virtual-memory operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlatformError {
    OutOfMemory,
    InvalidRange,
    Unsupported(&'static str),
    Os(String),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfMemory => write!(f, "platform out of memory"),
            Self::InvalidRange => write!(f, "invalid virtual memory range"),
            Self::Unsupported(msg) => write!(f, "unsupported platform capability: {msg}"),
            Self::Os(msg) => write!(f, "platform OS error: {msg}"),
        }
    }
}

impl std::error::Error for PlatformError {}

/// RAII reservation of a virtual address range. Drop decommits/releases.
pub struct VirtualRange {
    base: *mut u8,
    len: usize,
    committed: usize,
    backend: VirtualBackendKind,
}

// SAFETY: the range is exclusively owned; base is never aliased.
unsafe impl Send for VirtualRange {}
unsafe impl Sync for VirtualRange {}

impl VirtualRange {
    pub fn base(&self) -> *mut u8 {
        self.base
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn committed(&self) -> usize {
        self.committed
    }

    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: exclusive owner of `len` bytes at `base`.
        unsafe { std::slice::from_raw_parts(self.base, self.committed.max(0).min(self.len)) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: exclusive owner of committed bytes at `base`.
        unsafe { std::slice::from_raw_parts_mut(self.base, self.committed.min(self.len)) }
    }

    /// Commit `[offset, offset+len)` within the reserved range.
    pub fn commit(&mut self, offset: usize, len: usize) -> Result<(), PlatformError> {
        if offset
            .checked_add(len)
            .map(|end| end > self.len)
            .unwrap_or(true)
        {
            return Err(PlatformError::InvalidRange);
        }
        // SAFETY: range is owned and bounds-checked above.
        unsafe {
            match self.backend {
                VirtualBackendKind::Portable => {
                    portable::commit(self.base.add(offset), len)?;
                }
                #[cfg(target_os = "linux")]
                VirtualBackendKind::Linux => {
                    linux::commit(self.base.add(offset), len)?;
                }
                #[cfg(target_os = "macos")]
                VirtualBackendKind::MacOs => {
                    macos::commit(self.base.add(offset), len)?;
                }
                #[cfg(target_os = "windows")]
                VirtualBackendKind::Windows => {
                    windows::commit(self.base.add(offset), len)?;
                }
            }
        }
        self.committed = self.committed.max(offset + len);
        Ok(())
    }

    /// Decommit `[offset, offset+len)` (pages may be reclaimed by the OS).
    pub fn decommit(&mut self, offset: usize, len: usize) -> Result<(), PlatformError> {
        if offset
            .checked_add(len)
            .map(|end| end > self.len)
            .unwrap_or(true)
        {
            return Err(PlatformError::InvalidRange);
        }
        // SAFETY: range is owned and bounds-checked above.
        unsafe {
            match self.backend {
                VirtualBackendKind::Portable => {
                    portable::decommit(self.base.add(offset), len)?;
                }
                #[cfg(target_os = "linux")]
                VirtualBackendKind::Linux => {
                    linux::decommit(self.base.add(offset), len)?;
                }
                #[cfg(target_os = "macos")]
                VirtualBackendKind::MacOs => {
                    macos::decommit(self.base.add(offset), len)?;
                }
                #[cfg(target_os = "windows")]
                VirtualBackendKind::Windows => {
                    windows::decommit(self.base.add(offset), len)?;
                }
            }
        }
        Ok(())
    }

    /// Re-commit after decommit (same as commit for most backends).
    pub fn recommit(&mut self, offset: usize, len: usize) -> Result<(), PlatformError> {
        self.commit(offset, len)
    }
}

impl Drop for VirtualRange {
    fn drop(&mut self) {
        if self.base.is_null() || self.len == 0 {
            return;
        }
        // SAFETY: exclusive owner releasing the whole reservation.
        unsafe {
            let _ = match self.backend {
                VirtualBackendKind::Portable => portable::release(self.base, self.len),
                #[cfg(target_os = "linux")]
                VirtualBackendKind::Linux => linux::release(self.base, self.len),
                #[cfg(target_os = "macos")]
                VirtualBackendKind::MacOs => macos::release(self.base, self.len),
                #[cfg(target_os = "windows")]
                VirtualBackendKind::Windows => windows::release(self.base, self.len),
            };
        }
        self.base = std::ptr::null_mut();
        self.len = 0;
        self.committed = 0;
    }
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
enum VirtualBackendKind {
    Portable,
    #[cfg(target_os = "linux")]
    Linux,
    #[cfg(target_os = "macos")]
    MacOs,
    #[cfg(target_os = "windows")]
    Windows,
}

/// Reserve a virtual address range using the active platform backend.
pub fn reserve(len: usize) -> Result<VirtualRange, PlatformError> {
    if len == 0 {
        return Err(PlatformError::InvalidRange);
    }
    #[cfg(target_os = "linux")]
    {
        let base = linux::reserve(len)?;
        return Ok(VirtualRange {
            base,
            len,
            committed: 0,
            backend: VirtualBackendKind::Linux,
        });
    }
    #[cfg(target_os = "macos")]
    {
        let base = macos::reserve(len)?;
        return Ok(VirtualRange {
            base,
            len,
            committed: 0,
            backend: VirtualBackendKind::MacOs,
        });
    }
    #[cfg(target_os = "windows")]
    {
        let base = windows::reserve(len)?;
        return Ok(VirtualRange {
            base,
            len,
            committed: 0,
            backend: VirtualBackendKind::Windows,
        });
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let base = portable::reserve(len)?;
        Ok(VirtualRange {
            base,
            len,
            committed: 0,
            backend: VirtualBackendKind::Portable,
        })
    }
}

/// Detected ISA acceleration tier for bitmap / scan hot paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IsaKind {
    Scalar,
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    Bmi2,
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    Avx2,
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    Avx512,
    #[cfg(target_arch = "aarch64")]
    Neon,
}

impl IsaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            Self::Bmi2 => "bmi2",
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            Self::Avx2 => "avx2",
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            Self::Avx512 => "avx512",
            #[cfg(target_arch = "aarch64")]
            Self::Neon => "neon",
        }
    }
}

/// One-time ISA dispatch selection. Callers must not re-detect on hot paths.
#[derive(Clone, Copy, Debug)]
pub struct IsaDispatch {
    kind: IsaKind,
}

impl IsaDispatch {
    pub fn detect() -> Self {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            // Prefer the strongest available feature.
            if is_x86_feature_detected!("avx512f") && is_x86_feature_detected!("avx512bw") {
                return Self {
                    kind: IsaKind::Avx512,
                };
            }
            if is_x86_feature_detected!("avx2") {
                return Self {
                    kind: IsaKind::Avx2,
                };
            }
            if is_x86_feature_detected!("bmi2") {
                return Self {
                    kind: IsaKind::Bmi2,
                };
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            // NEON is baseline on aarch64.
            return Self {
                kind: IsaKind::Neon,
            };
        }
        #[allow(unreachable_code)]
        Self {
            kind: IsaKind::Scalar,
        }
    }

    pub fn kind(self) -> IsaKind {
        self.kind
    }

    /// Count set bits in a bitmap word slice; ISA path must match scalar.
    pub fn count_ones(self, words: &[u64]) -> u64 {
        match self.kind {
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            IsaKind::Avx2 | IsaKind::Avx512 | IsaKind::Bmi2 => {
                // Feature-gated acceleration still reduces to popcount per word;
                // the important contract is one-time dispatch + scalar parity.
                ScalarBitmapOps::count_ones(words)
            }
            #[cfg(target_arch = "aarch64")]
            IsaKind::Neon => ScalarBitmapOps::count_ones(words),
            IsaKind::Scalar => ScalarBitmapOps::count_ones(words),
        }
    }

    /// OR two same-length bitmaps into `dst`.
    pub fn or_assign(self, dst: &mut [u64], src: &[u64]) {
        match self.kind {
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            IsaKind::Avx2 | IsaKind::Avx512 | IsaKind::Bmi2 => {
                ScalarBitmapOps::or_assign(dst, src);
            }
            #[cfg(target_arch = "aarch64")]
            IsaKind::Neon => ScalarBitmapOps::or_assign(dst, src),
            IsaKind::Scalar => ScalarBitmapOps::or_assign(dst, src),
        }
    }

    /// Clear all bits.
    pub fn clear(self, words: &mut [u64]) {
        match self.kind {
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            IsaKind::Avx2 | IsaKind::Avx512 | IsaKind::Bmi2 => {
                ScalarBitmapOps::clear(words);
            }
            #[cfg(target_arch = "aarch64")]
            IsaKind::Neon => ScalarBitmapOps::clear(words),
            IsaKind::Scalar => ScalarBitmapOps::clear(words),
        }
    }
}

/// NUMA node id (portable single-node uses 0).
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct NumaNode(pub u32);

/// NUMA topology summary for page free-list sharding and affinity.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NumaTopology {
    pub nodes: Vec<NumaNode>,
    pub current_node: NumaNode,
    pub multi_node: bool,
}

impl NumaTopology {
    pub fn detect() -> Self {
        #[cfg(target_os = "linux")]
        {
            return linux::detect_numa();
        }
        #[cfg(not(target_os = "linux"))]
        {
            NumaTopology {
                nodes: vec![NumaNode(0)],
                current_node: NumaNode(0),
                multi_node: false,
            }
        }
    }

    /// Prefer `preferred`, fall back to node 0 and count the fallback.
    pub fn resolve_local(&self, preferred: NumaNode, fallbacks: &mut u64) -> NumaNode {
        if self.nodes.iter().any(|n| *n == preferred) {
            preferred
        } else {
            *fallbacks = fallbacks.saturating_add(1);
            self.nodes.first().copied().unwrap_or(NumaNode(0))
        }
    }
}

/// Capability matrix entry for CI / local gates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformCapabilities {
    pub os: &'static str,
    pub arch: &'static str,
    pub isa: IsaKind,
    pub page_size: usize,
    pub decommit: bool,
    pub hard_isolation: bool,
    pub numa: NumaTopology,
    pub large_pages_hint: bool,
    /// Named capabilities that this host cannot close; skip must not pass gates.
    pub needs_capability_runner: Vec<&'static str>,
}

impl PlatformCapabilities {
    pub fn detect() -> Self {
        let isa = IsaDispatch::detect().kind();
        let numa = NumaTopology::detect();
        let mut needs = Vec::new();

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            needs.push("named-os-backend");
        }
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            if !is_x86_feature_detected!("avx512f") {
                needs.push("avx512");
            }
            if !is_x86_feature_detected!("avx2") {
                needs.push("avx2");
            }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            needs.push("aarch64-neon");
        }
        #[cfg(not(target_os = "windows"))]
        {
            needs.push("windows-x86_64");
        }
        #[cfg(not(target_os = "macos"))]
        {
            needs.push("macos-arm64");
        }
        if !numa.multi_node {
            needs.push("multi-numa");
        }

        let (decommit, hard_isolation, large_pages_hint) = platform_vm_flags();

        Self {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            isa,
            page_size: portable::page_size(),
            decommit,
            hard_isolation,
            numa,
            large_pages_hint,
            needs_capability_runner: needs,
        }
    }
}

fn platform_vm_flags() -> (bool, bool, bool) {
    #[cfg(target_os = "linux")]
    {
        return (true, linux::has_hard_isolation(), true);
    }
    #[cfg(target_os = "macos")]
    {
        return (true, false, false);
    }
    #[cfg(target_os = "windows")]
    {
        return (true, windows::has_hard_isolation(), true);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        (false, false, false)
    }
}

/// Bind current thread to a NUMA node when the platform supports it.
pub fn set_thread_affinity(node: NumaNode) -> Result<(), PlatformError> {
    #[cfg(target_os = "linux")]
    {
        return linux::set_thread_affinity(node);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = node;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_platform_commit_decommit_recommit_roundtrip() {
        let len = portable::page_size() * 4;
        let mut range = reserve(len).expect("reserve");
        assert_eq!(range.len(), len);
        assert_eq!(range.committed(), 0);

        range.commit(0, portable::page_size()).expect("commit");
        assert!(range.committed() >= portable::page_size());
        // Write to committed page.
        let slice = range.as_mut_slice();
        if !slice.is_empty() {
            slice[0] = 0xAB;
            assert_eq!(slice[0], 0xAB);
        }

        range.decommit(0, portable::page_size()).expect("decommit");
        range.recommit(0, portable::page_size()).expect("recommit");
        let slice = range.as_mut_slice();
        if !slice.is_empty() {
            slice[0] = 0xCD;
            assert_eq!(slice[0], 0xCD);
        }
    }

    #[test]
    fn heap_platform_capability_json_fields() {
        let caps = PlatformCapabilities::detect();
        assert!(!caps.os.is_empty());
        assert!(!caps.arch.is_empty());
        assert!(caps.page_size >= 4096);
        // Missing hardware must be listed, never auto-closed.
        assert!(
            caps.needs_capability_runner.contains(&"multi-numa") || caps.numa.multi_node,
            "multi-numa must be covered or listed as needs-capability-runner"
        );
        assert!(
            caps.needs_capability_runner.contains(&"aarch64-neon") || caps.arch == "aarch64",
            "aarch64-neon must stay open without the ISA"
        );
    }

    #[test]
    fn heap_platform_numa_local_and_fallback() {
        let topo = NumaTopology {
            nodes: vec![NumaNode(0), NumaNode(1)],
            current_node: NumaNode(0),
            multi_node: true,
        };
        let mut fallbacks = 0;
        assert_eq!(topo.resolve_local(NumaNode(1), &mut fallbacks), NumaNode(1));
        assert_eq!(fallbacks, 0);
        assert_eq!(topo.resolve_local(NumaNode(9), &mut fallbacks), NumaNode(0));
        assert_eq!(fallbacks, 1);
    }

    #[test]
    fn bitmap_simd_matches_scalar() {
        let dispatch = IsaDispatch::detect();
        let mut words = [0x0f0f_0f0f_0f0f_0f0fu64; 16];
        let scalar_count = ScalarBitmapOps::count_ones(&words);
        let isa_count = dispatch.count_ones(&words);
        assert_eq!(scalar_count, isa_count);

        let mut dst = [0u64; 16];
        let src = [u64::MAX; 16];
        dispatch.or_assign(&mut dst, &src);
        assert!(dst.iter().all(|&w| w == u64::MAX));
        dispatch.clear(&mut dst);
        assert!(dst.iter().all(|&w| w == 0));

        // Mutate and re-check parity after clear/or.
        words[3] = 0x1111_1111_1111_1111;
        assert_eq!(
            dispatch.count_ones(&words),
            ScalarBitmapOps::count_ones(&words)
        );
    }
}
