use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::cli::Profile;

#[path = "resource_platform.rs"]
mod resource_platform;

pub const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;
const HANDLE_REGION_BYTES: u64 = 32 * GIB;
const CONTROL_REGION_BYTES: u64 = 64 * MIB;
const WASMTIME_GUARD_BYTES: u64 = 2 * GIB;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HostResourceSnapshot {
    pub physical_total_bytes: u64,
    pub mem_available_bytes: u64,
    pub cgroup_path: Option<String>,
    pub cgroup_memory_controller: Option<bool>,
    pub cgroup_delegated: Option<bool>,
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
    pub probe_errors: Vec<String>,
}

#[derive(Default)]
pub(super) struct PlatformProbe {
    pub(super) cgroup_path: Option<String>,
    pub(super) cgroup_memory_controller: Option<bool>,
    pub(super) cgroup_delegated: Option<bool>,
    pub(super) cgroup_limit_bytes: Option<u64>,
    pub(super) cgroup_current_bytes: Option<u64>,
    pub(super) job_limit_bytes: Option<u64>,
    pub(super) job_current_bytes: Option<u64>,
    pub(super) address_space_limit_bytes: Option<u64>,
    pub(super) swap_total_bytes: Option<u64>,
    pub(super) psi_full_avg10: Option<f64>,
    pub(super) hard_isolation: bool,
    pub(super) isolation_kind: Option<String>,
    pub(super) errors: Vec<String>,
}

impl HostResourceSnapshot {
    pub fn synthetic(physical_total_bytes: u64, mem_available_bytes: u64) -> Self {
        Self::from_probe(
            physical_total_bytes,
            mem_available_bytes,
            PlatformProbe::default(),
        )
    }

    pub(super) fn from_probe(
        physical_total_bytes: u64,
        mem_available_bytes: u64,
        probe: PlatformProbe,
    ) -> Self {
        let limit = finite_min(probe.cgroup_limit_bytes, probe.job_limit_bytes);
        let remaining = finite_remaining(probe.cgroup_limit_bytes, probe.cgroup_current_bytes)
            .into_iter()
            .chain(finite_remaining(
                probe.job_limit_bytes,
                probe.job_current_bytes,
            ))
            .min();
        Self {
            physical_total_bytes,
            mem_available_bytes,
            cgroup_path: probe.cgroup_path,
            cgroup_memory_controller: probe.cgroup_memory_controller,
            cgroup_delegated: probe.cgroup_delegated,
            cgroup_limit_bytes: probe.cgroup_limit_bytes,
            cgroup_current_bytes: probe.cgroup_current_bytes,
            job_limit_bytes: probe.job_limit_bytes,
            job_current_bytes: probe.job_current_bytes,
            address_space_limit_bytes: probe.address_space_limit_bytes,
            swap_total_bytes: probe.swap_total_bytes,
            psi_full_avg10: probe.psi_full_avg10,
            effective_total_bytes: limit.map_or(physical_total_bytes, |value| {
                value.min(physical_total_bytes)
            }),
            effective_available_bytes: remaining
                .map_or(mem_available_bytes, |value| value.min(mem_available_bytes)),
            hard_isolation: probe.hard_isolation,
            isolation_kind: probe.isolation_kind,
            probe_errors: probe.errors,
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
        Ok(resource_platform::snapshot(
            system.total_memory(),
            system.available_memory(),
        ))
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
    let mut reasons = resources
        .probe_errors
        .iter()
        .map(|error| format!("资源探测不可验证：{error}"))
        .collect::<Vec<_>>();

    if resources.effective_total_bytes < required_total_bytes {
        reasons.push(format!(
            "effective_total={} 小于 required_total={required_total_bytes}",
            resources.effective_total_bytes
        ));
    }
    if resources.effective_available_bytes < required_available_bytes {
        reasons.push(format!(
            "effective_available={} 小于 required_available={required_available_bytes}",
            resources.effective_available_bytes
        ));
    }
    if let Some(limit) = resources.address_space_limit_bytes
        && limit < required_virtual_address_bytes
    {
        reasons.push(format!(
            "address_space_limit={limit} 无法保留 required_virtual_address={required_virtual_address_bytes}"
        ));
    }
    if request.profile == Profile::Nightly && !resources.hard_isolation {
        reasons.push(
            "nightly profile 需要 delegated cgroup v2 或 Windows Job Object hard isolation".into(),
        );
    }

    AdmissionDecision {
        status: reasons
            .is_empty()
            .then_some(AdmissionStatus::Admitted)
            .unwrap_or(AdmissionStatus::NeedsResourceRunner),
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

#[cfg(test)]
mod tests {
    use super::*;

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
