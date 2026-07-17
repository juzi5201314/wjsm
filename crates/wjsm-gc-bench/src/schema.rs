use serde::{Deserialize, Serialize};

use crate::resource::{AdmissionDecision, HostResourceSnapshot};
use crate::scenario::{Denominators, ScenarioManifest};
use crate::stats::DistributionSummary;

pub const BENCHMARK_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateStatus {
    Passed,
    Failed,
    NeedsVerification,
    NeedsResourceRunner,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CounterSource {
    pub name: String,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MetricObservation {
    pub name: String,
    pub numerator: Option<f64>,
    pub denominator: Option<f64>,
    pub value: Option<f64>,
    pub source: CounterSource,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeMetadata {
    pub engine: String,
    pub gc: String,
    pub tool_version: String,
    pub wasmtime_version: String,
    pub hardware: HardwareMetadata,
    pub jdk_probe_patch_sha256: Option<String>,
    pub counter_source: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HardwareMetadata {
    pub architecture: String,
    pub os: String,
    pub logical_cpus: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SampleReport {
    pub index: usize,
    pub steady_state_ns: u64,
    pub gc_telemetry: wjsm_runtime::GcTelemetrySnapshot,
    pub metrics: Vec<MetricObservation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunConfiguration {
    pub samples: usize,
    pub duration_seconds: u64,
    pub workers: usize,
    pub relocate_every_page: bool,
    pub barrier_buffer_capacity: usize,
    pub safepoint_every_allocation: bool,
    pub jdk_home: Option<std::path::PathBuf>,
    pub jdk_probe_home: Option<std::path::PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunReport {
    pub schema_version: u32,
    pub status: GateStatus,
    pub runtime: RuntimeMetadata,
    pub scenario: ScenarioManifest,
    pub denominators: Denominators,
    pub resources: HostResourceSnapshot,
    pub admission: AdmissionDecision,
    pub configuration: RunConfiguration,
    pub samples: Vec<SampleReport>,
    pub steady_state: DistributionSummary,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BenchmarkManifest {
    pub schema_version: u32,
    pub status: GateStatus,
    pub reports: Vec<RunReport>,
    pub notes: Vec<String>,
}

impl BenchmarkManifest {
    pub fn empty() -> Self {
        Self {
            schema_version: BENCHMARK_SCHEMA_VERSION,
            status: GateStatus::NeedsVerification,
            notes: Vec::new(),
            reports: Vec::new(),
        }
    }
}
