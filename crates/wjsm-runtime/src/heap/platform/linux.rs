//! Linux virtual memory (mmap/madvise) and NUMA topology.

use super::{NumaNode, NumaTopology, PlatformError};

use std::fs;
use std::io;
use std::ptr;

pub struct LinuxVirtualMemory;

pub fn reserve(len: usize) -> Result<*mut u8, PlatformError> {
    // SAFETY: anonymous private mapping of `len` bytes.
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
    // SAFETY: caller owns the mapping.
    let rc = unsafe { libc::mprotect(base.cast(), len, libc::PROT_READ | libc::PROT_WRITE) };
    if rc != 0 {
        return Err(PlatformError::Os(io::Error::last_os_error().to_string()));
    }
    Ok(())
}

/// SAFETY: `base` is a previously reserved range of at least `len` bytes.
pub unsafe fn decommit(base: *mut u8, len: usize) -> Result<(), PlatformError> {
    // SAFETY: caller owns the mapping; MADV_DONTNEED allows reclaim.
    let rc = unsafe { libc::madvise(base.cast(), len, libc::MADV_DONTNEED) };
    if rc != 0 {
        return Err(PlatformError::Os(io::Error::last_os_error().to_string()));
    }
    let rc = unsafe { libc::mprotect(base.cast(), len, libc::PROT_NONE) };
    if rc != 0 {
        return Err(PlatformError::Os(io::Error::last_os_error().to_string()));
    }
    Ok(())
}

/// SAFETY: `base` is a previously reserved range of `len` bytes.
pub unsafe fn release(base: *mut u8, len: usize) -> Result<(), PlatformError> {
    // SAFETY: full mapping release.
    let rc = unsafe { libc::munmap(base.cast(), len) };
    if rc != 0 {
        Err(PlatformError::Os(io::Error::last_os_error().to_string()))
    } else {
        Ok(())
    }
}

pub fn has_hard_isolation() -> bool {
    // cgroup v2 memory controller is the hard-isolation signal for Task 23/25.
    fs::read_to_string("/proc/self/cgroup")
        .map(|s| s.lines().any(|line| line.starts_with("0::")))
        .unwrap_or(false)
        && fs::read_to_string("/sys/fs/cgroup/cgroup.controllers")
            .map(|s| s.split_whitespace().any(|c| c == "memory"))
            .unwrap_or(false)
}

pub fn detect_numa() -> NumaTopology {
    let mut nodes = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/devices/system/node") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(rest) = name.strip_prefix("node") {
                if let Ok(id) = rest.parse::<u32>() {
                    // Only include online nodes with CPUs when possible.
                    let online = entry.path().join("cpulist");
                    if online.exists() {
                        nodes.push(NumaNode(id));
                    }
                }
            }
        }
    }
    nodes.sort();
    nodes.dedup();
    if nodes.is_empty() {
        nodes.push(NumaNode(0));
    }
    let multi_node = nodes.len() > 1;
    let current_node = current_cpu_node().unwrap_or(nodes[0]);
    NumaTopology {
        nodes,
        current_node,
        multi_node,
    }
}

fn current_cpu_node() -> Option<NumaNode> {
    let cpu = unsafe { libc::sched_getcpu() };
    if cpu < 0 {
        return None;
    }
    let path = format!("/sys/devices/system/cpu/cpu{cpu}/node");
    // Some kernels expose node via symlink under cpuN.
    if let Ok(meta) = fs::read_link(format!("/sys/devices/system/cpu/cpu{cpu}/node*")) {
        let _ = meta;
    }
    // Walk /sys/devices/system/node/node*/cpulist
    if let Ok(entries) = fs::read_dir("/sys/devices/system/node") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(rest) = name.strip_prefix("node") {
                if let Ok(id) = rest.parse::<u32>() {
                    if let Ok(list) = fs::read_to_string(entry.path().join("cpulist")) {
                        if cpu_in_list(cpu as u32, list.trim()) {
                            return Some(NumaNode(id));
                        }
                    }
                }
            }
        }
    }
    let _ = path;
    Some(NumaNode(0))
}

fn cpu_in_list(cpu: u32, list: &str) -> bool {
    for part in list.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(start), Ok(end)) = (a.parse::<u32>(), b.parse::<u32>()) {
                if (start..=end).contains(&cpu) {
                    return true;
                }
            }
        } else if part.parse::<u32>() == Ok(cpu) {
            return true;
        }
    }
    false
}

pub fn set_thread_affinity(node: NumaNode) -> Result<(), PlatformError> {
    // Soft affinity via cpulist of the node; failure is non-fatal for portable hosts.
    let path = format!("/sys/devices/system/node/node{}/cpulist", node.0);
    let list = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    let mut any = false;
    for part in list.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(start), Ok(end)) = (a.parse::<usize>(), b.parse::<usize>()) {
                for cpu in start..=end {
                    unsafe {
                        libc::CPU_SET(cpu, &mut set);
                    }
                    any = true;
                }
            }
        } else if let Ok(cpu) = part.parse::<usize>() {
            unsafe {
                libc::CPU_SET(cpu, &mut set);
            }
            any = true;
        }
    }
    if !any {
        return Ok(());
    }
    let rc = unsafe {
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set)
    };
    if rc != 0 {
        Err(PlatformError::Os(io::Error::last_os_error().to_string()))
    } else {
        Ok(())
    }
}
