use serde::{Deserialize, Serialize};

use crate::resource::HostInfo;

pub const BENCHMARK_SCHEMA_VERSION: u32 = 2;

/// 单个样本的原始数据。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SampleReport {
    pub index: usize,
    /// user main() 执行纳秒（不含 compile/instantiate/startup）。
    pub steady_state_ns: u64,
    /// runtime 提供的完整 GC telemetry 快照。
    pub telemetry: wjsm_runtime::GcTelemetrySnapshot,
}

/// 分布统计摘要。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Distribution {
    pub count: usize,
    pub mean: Option<f64>,
    pub min: Option<u64>,
    pub p50: Option<u64>,
    pub p99: Option<u64>,
    pub max: Option<u64>,
}

/// 从 telemetry 聚合计算的派生指标。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DerivedMetrics {
    /// GC CPU 时间 / 物理分配字节。
    pub gc_cpu_per_allocated_byte: Option<f64>,
    /// mark CPU 时间 / mark 存活字节。
    pub mark_cpu_per_live_byte: Option<f64>,
    /// relocation CPU 时间 / 迁移字节。
    pub relocation_cpu_per_relocated_byte: Option<f64>,
    /// 分配速率 bytes/s（物理分配 / steady-state 时间）。
    pub allocation_rate_bytes_per_sec: Option<f64>,
    /// GC CPU 占 steady-state 时间的百分比。
    pub gc_overhead_percent: Option<f64>,
    /// barrier load 事件 / steady-state 秒。
    pub barrier_load_events_per_sec: Option<f64>,
    /// barrier store 事件 / steady-state 秒。
    pub barrier_store_events_per_sec: Option<f64>,
    /// 每秒 GC 周期数。
    pub gc_cycles_per_sec: Option<f64>,
}

/// 完整基准报告。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BenchReport {
    pub schema_version: u32,
    pub config: BenchConfig,
    pub hardware: HostInfo,
    pub samples: Vec<SampleReport>,
    pub summary: BenchSummary,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BenchConfig {
    pub gc: String,
    pub heap_bytes: u64,
    pub scenario: String,
    pub live_set_percent: u8,
    pub samples: usize,
    pub duration_seconds: u64,
    pub seed: u64,
    pub allocations: u64,
    pub retained: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct BenchSummary {
    pub steady_state_ns: Distribution,
    pub gc_cpu_ns: Distribution,
    pub pause_max_ns: Distribution,
    pub metrics: DerivedMetrics,
    /// 所有样本 telemetry 的聚合累计。
    pub telemetry_totals: wjsm_runtime::GcTelemetrySnapshot,
}
