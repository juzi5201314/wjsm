use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::cli::Profile;

pub const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;
const HANDLE_REGION_BYTES: u64 = 32 * GIB;
const CONTROL_REGION_BYTES: u64 = 64 * MIB;
const WASMTIME_GUARD_BYTES: u64 = 2 * GIB;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HostResourceSnapshot {
    pub physical_total_bytes: u64,
    pub mem_available_bytes: u64,
    pub cgroup_limit_bytes: Option<u64>,
    pub cgroup_current_bytes: Option<u64>,
    pub job_limit_bytes: Option<u64>,
    pub job_current_bytes: Option<u64>,
    pub address_space_limit_bytes: Option<u64>,
    pub swap_total_bytes: Option<u64>,
    pub psi_full_avg10: Option<f64>,
    pub effective_total_bytes: u64,
    pub effective_available_bytes: u64,
    pub hard_isolation: bool,
    pub isolation_kind: Option<String>,
}

impl HostResourceSnapshot {
    pub fn synthetic(physical_total_bytes: u64, mem_available_bytes: u64) -> Self {
        Self::new(
            physical_total_bytes,
            mem_available_bytes,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        physical_total_bytes: u64,
        mem_available_bytes: u64,
        cgroup_limit_bytes: Option<u64>,
        cgroup_current_bytes: Option<u64>,
        job_limit_bytes: Option<u64>,
        job_current_bytes: Option<u64>,
        address_space_limit_bytes: Option<u64>,
        swap_total_bytes: Option<u64>,
        psi_full_avg10: Option<f64>,
        hard_isolation: bool,
        isolation_kind: Option<String>,
    ) -> Self {
        let limit = finite_min(cgroup_limit_bytes, job_limit_bytes);
        let remaining = finite_remaining(cgroup_limit_bytes, cgroup_current_bytes)
            .into_iter()
            .chain(finite_remaining(job_limit_bytes, job_current_bytes))
            .min();
        Self {
            physical_total_bytes,
            mem_available_bytes,
            cgroup_limit_bytes,
            cgroup_current_bytes,
            job_limit_bytes,
            job_current_bytes,
            address_space_limit_bytes,
            swap_total_bytes,
            psi_full_avg10,
            effective_total_bytes: limit.map_or(physical_total_bytes, |value| {
                value.min(physical_total_bytes)
            }),
            effective_available_bytes: remaining
                .map_or(mem_available_bytes, |value| value.min(mem_available_bytes)),
            hard_isolation,
            isolation_kind,
        }
    }
}

pub trait HostResourceProvider {
    fn snapshot(&self) -> Result<HostResourceSnapshot>;
}

pub struct SystemHostResourceProvider;

impl HostResourceProvider for SystemHostResourceProvider {
    fn snapshot(&self) -> Result<HostResourceSnapshot> {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        let physical_total_bytes = system.total_memory();
        let mem_available_bytes = system.available_memory();
        platform_snapshot(physical_total_bytes, mem_available_bytes)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AdmissionRequest {
    pub heap_cap_bytes: u64,
    pub profile: Profile,
}

impl AdmissionRequest {
    pub fn pr(heap_cap_bytes: u64) -> Self {
        Self {
            heap_cap_bytes,
            profile: Profile::Pr,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdmissionStatus {
    Admitted,
    NeedsResourceRunner,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BudgetFormulas {
    pub effective_total: String,
    pub effective_available: String,
    pub required_total: String,
    pub required_available: String,
    pub required_virtual_address: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AdmissionDecision {
    pub status: AdmissionStatus,
    pub required_total_bytes: u64,
    pub required_available_bytes: u64,
    pub required_virtual_address_bytes: u64,
    pub budget_formulas: BudgetFormulas,
    pub reasons: Vec<String>,
}

pub fn admit(resources: &HostResourceSnapshot, request: AdmissionRequest) -> AdmissionDecision {
    let required_total_bytes = request.heap_cap_bytes.saturating_mul(4);
    let availability_reserve = (resources.effective_total_bytes / 10).max(2 * GIB);
    let required_available_bytes = request
        .heap_cap_bytes
        .saturating_mul(3)
        .saturating_add(availability_reserve);
    let required_virtual_address_bytes = HANDLE_REGION_BYTES
        .saturating_add(CONTROL_REGION_BYTES)
        .saturating_add(request.heap_cap_bytes)
        .saturating_add(WASMTIME_GUARD_BYTES);
    let mut reasons = Vec::new();

    if resources.effective_total_bytes < required_total_bytes {
        reasons.push(format!(
            "effective_total={} is below required_total={required_total_bytes}",
            resources.effective_total_bytes
        ));
    }
    if resources.effective_available_bytes < required_available_bytes {
        reasons.push(format!(
            "effective_available={} is below required_available={required_available_bytes}",
            resources.effective_available_bytes
        ));
    }
    if let Some(limit) = resources.address_space_limit_bytes
        && limit < required_virtual_address_bytes
    {
        reasons.push(format!(
            "address_space_limit={limit} cannot reserve required_virtual_address={required_virtual_address_bytes}"
        ));
    }
    if request.profile == Profile::Nightly && !resources.hard_isolation {
        reasons.push(
            "nightly profile requires delegated cgroup v2 or Windows Job Object hard isolation"
                .into(),
        );
    }

    let status = if reasons.is_empty() {
        AdmissionStatus::Admitted
    } else {
        AdmissionStatus::NeedsResourceRunner
    };
    AdmissionDecision {
        status,
        required_total_bytes,
        required_available_bytes,
        required_virtual_address_bytes,
        budget_formulas: BudgetFormulas {
            effective_total: "min(physical_total, finite(cgroup_limit, job_limit))".into(),
            effective_available:
                "min(MemAvailable, finite(cgroup_limit-cgroup_current, job_limit-job_current))"
                    .into(),
            required_total: "4 * max_heap_cap".into(),
            required_available: "3 * max_heap_cap + max(2 GiB, 10% * effective_total)".into(),
            required_virtual_address:
                "32 GiB handle region + 64 MiB control + max_heap_cap + 2 GiB Wasmtime guards"
                    .into(),
        },
        reasons,
    }
}

pub fn admit_then_run<T>(
    provider: &dyn HostResourceProvider,
    request: AdmissionRequest,
    run: impl FnOnce() -> Result<T>,
) -> Result<AdmissionDecision> {
    let decision = admit(&provider.snapshot()?, request);
    if decision.status == AdmissionStatus::Admitted {
        run()?;
    }
    Ok(decision)
}

fn finite_min(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    left.into_iter().chain(right).min()
}

fn finite_remaining(limit: Option<u64>, current: Option<u64>) -> Option<u64> {
    limit
        .zip(current)
        .map(|(limit, current)| limit.saturating_sub(current))
}

#[cfg(target_os = "linux")]
fn platform_snapshot(
    physical_total_bytes: u64,
    mem_available_bytes: u64,
) -> Result<HostResourceSnapshot> {
    let meminfo = fs::read_to_string("/proc/meminfo").context("read /proc/meminfo")?;
    let values = parse_meminfo(&meminfo)?;
    let mem_available_bytes = values
        .get("MemAvailable")
        .copied()
        .unwrap_or(mem_available_bytes);
    let cgroup_limit_bytes = read_limit(Path::new("/sys/fs/cgroup/memory.max"))?;
    let cgroup_current_bytes = read_u64(Path::new("/sys/fs/cgroup/memory.current"))?;
    let psi_full_avg10 = read_psi_full_avg10(Path::new("/proc/pressure/memory"))?;
    let address_space_limit_bytes = rlimit_as()?;
    let hard_isolation = cgroup_limit_bytes.is_some();
    Ok(HostResourceSnapshot::new(
        physical_total_bytes,
        mem_available_bytes,
        cgroup_limit_bytes,
        cgroup_current_bytes,
        None,
        None,
        address_space_limit_bytes,
        values.get("SwapTotal").copied(),
        psi_full_avg10,
        hard_isolation,
        hard_isolation.then(|| "cgroup-v2".into()),
    ))
}

#[cfg(target_os = "linux")]
fn parse_meminfo(input: &str) -> Result<std::collections::BTreeMap<String, u64>> {
    let mut values = std::collections::BTreeMap::new();
    for line in input.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let mut parts = value.split_whitespace();
        let Some(number) = parts.next() else {
            continue;
        };
        let unit = parts.next().unwrap_or("B");
        let multiplier = match unit {
            "kB" => 1024,
            "B" => 1,
            unexpected => anyhow::bail!("unsupported meminfo unit `{unexpected}`"),
        };
        values.insert(
            key.to_owned(),
            number
                .parse::<u64>()
                .with_context(|| format!("parse /proc/meminfo value for {key}"))?
                .saturating_mul(multiplier),
        );
    }
    Ok(values)
}

#[cfg(target_os = "linux")]
fn read_limit(path: &Path) -> Result<Option<u64>> {
    let value = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let value = value.trim();
    if value == "max" {
        return Ok(None);
    }
    value
        .parse::<u64>()
        .map(Some)
        .with_context(|| format!("parse {}", path.display()))
}

#[cfg(target_os = "linux")]
fn read_u64(path: &Path) -> Result<Option<u64>> {
    match fs::read_to_string(path) {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .map(Some)
            .with_context(|| format!("parse {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

#[cfg(target_os = "linux")]
fn read_psi_full_avg10(path: &Path) -> Result<Option<f64>> {
    let input = match fs::read_to_string(path) {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let value = input
        .lines()
        .find(|line| line.starts_with("full "))
        .and_then(|line| {
            line.split_whitespace()
                .find(|part| part.starts_with("avg10="))
        })
        .and_then(|part| part.strip_prefix("avg10="))
        .map(str::parse)
        .transpose()
        .context("parse /proc/pressure/memory full avg10")?;
    Ok(value)
}

#[cfg(target_os = "linux")]
fn rlimit_as() -> Result<Option<u64>> {
    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: `limit` points to valid writable storage for libc's `getrlimit` output.
    let result = unsafe { libc::getrlimit(libc::RLIMIT_AS, limit.as_mut_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("getrlimit(RLIMIT_AS)");
    }
    // SAFETY: a zero return from `getrlimit` initializes the entire `rlimit` structure.
    let limit = unsafe { limit.assume_init() };
    if limit.rlim_cur == libc::RLIM_INFINITY {
        Ok(None)
    } else {
        Ok(Some(limit.rlim_cur))
    }
}

#[cfg(target_os = "windows")]
fn platform_snapshot(
    physical_total_bytes: u64,
    mem_available_bytes: u64,
) -> Result<HostResourceSnapshot> {
    let (job_limit_bytes, job_current_bytes, hard_isolation) = windows_job_memory_limit()?;
    Ok(HostResourceSnapshot::new(
        physical_total_bytes,
        mem_available_bytes,
        None,
        None,
        job_limit_bytes,
        job_current_bytes,
        None,
        None,
        None,
        hard_isolation,
        hard_isolation.then(|| "windows-job-object".into()),
    ))
}

#[cfg(target_os = "windows")]
fn windows_job_memory_limit() -> Result<(Option<u64>, Option<u64>, bool)> {
    use std::mem::size_of;
    use windows_sys::Win32::System::JobObjects::{
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, QueryInformationJobObject,
    };

    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    // SAFETY: null job handle asks for the current process job; `info` is a valid output buffer.
    let success = unsafe {
        QueryInformationJobObject(
            std::ptr::null_mut(),
            JobObjectExtendedLimitInformation,
            (&mut info).cast(),
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .expect("JOB_OBJECT limit structure size fits u32"),
            std::ptr::null_mut(),
        )
    };
    if success == 0 {
        return Ok((None, None, false));
    }
    let limited = info.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0;
    let limit = limited.then_some(info.JobMemoryLimit as u64);
    let current = limited.then_some(info.PeakJobMemoryUsed as u64);
    Ok((limit, current, limited))
}

#[cfg(target_os = "macos")]
fn platform_snapshot(
    physical_total_bytes: u64,
    mem_available_bytes: u64,
) -> Result<HostResourceSnapshot> {
    Ok(HostResourceSnapshot::new(
        physical_total_bytes,
        mem_available_bytes,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        None,
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn platform_snapshot(
    physical_total_bytes: u64,
    mem_available_bytes: u64,
) -> Result<HostResourceSnapshot> {
    Ok(HostResourceSnapshot::synthetic(
        physical_total_bytes,
        mem_available_bytes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
    fn parse_meminfo_converts_kib_to_bytes() {
        let values = parse_meminfo("MemAvailable:       42 kB\nSwapTotal: 3 kB\n").unwrap();
        assert_eq!(values["MemAvailable"], 42 * 1024);
        assert_eq!(values["SwapTotal"], 3 * 1024);
    }

    #[test]
    #[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
    fn nightly_requires_hard_isolation() {
        let resources = HostResourceSnapshot::synthetic(128 * GIB, 128 * GIB);
        let decision = admit(
            &resources,
            AdmissionRequest {
                heap_cap_bytes: GIB,
                profile: Profile::Nightly,
            },
        );
        assert_eq!(decision.status, AdmissionStatus::NeedsResourceRunner);
    }
}
