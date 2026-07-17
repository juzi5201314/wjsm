mod histogram;
mod json;

use crate::{GcExecutionStats, GcStats};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use histogram::PauseHistogram;

pub const GC_TELEMETRY_SCHEMA_VERSION: u32 = 1;

/// HDR histogram 的稳定 JSON 快照。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct HistogramSnapshot {
    pub count: u64,
    pub min_ns: u64,
    pub max_ns: u64,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub p999_ns: u64,
}

/// runtime 在一次 benchmark 运行结束时提供的版本化 GC telemetry。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct GcTelemetrySnapshot {
    pub schema_version: u32,
    pub collector: String,
    pub cycles: u64,
    pub pause: HistogramSnapshot,
    pub reclaimed_bytes: u64,
    pub relocated_bytes: u64,
    pub marked_bytes: Option<u64>,
    pub promoted_bytes: Option<u64>,
    pub gc_cpu_ns: Option<u64>,
    pub mark_cpu_ns: Option<u64>,
    pub relocation_cpu_ns: Option<u64>,
    pub physical_allocated_bytes: Option<u64>,
    pub barrier_load_fast_events: Option<u64>,
    pub barrier_store_fast_events: Option<u64>,
    pub committed_handle_bytes: Option<u64>,
    pub committed_page_bytes: Option<u64>,
    pub committed_bitmap_bytes: Option<u64>,
    pub committed_remset_bytes: Option<u64>,
    pub committed_forwarding_bytes: Option<u64>,
    pub committed_worker_bytes: Option<u64>,
}

/// 可跨线程累计 GC 周期并生成不可变快照的唯一 telemetry owner。
#[derive(Default)]
pub struct GcTelemetry {
    inner: Mutex<TelemetryInner>,
}

#[derive(Default)]
struct TelemetryInner {
    collector: String,
    cycles: u64,
    pause: PauseHistogram,
    reclaimed_bytes: u64,
    relocated_bytes: u64,
}

impl GcTelemetry {
    pub fn record_cycle(&self, collector: &str, stats: &GcStats) {
        let mut inner = self.inner.lock().expect("GC telemetry lock poisoned");
        if inner.collector.is_empty() {
            inner.collector.push_str(collector);
        } else {
            assert_eq!(inner.collector, collector, "telemetry collector changed");
        }
        inner.cycles += 1;
        if stats.pause_count > 0 {
            inner.pause.record(stats.pause_ns_max);
        }
        inner.reclaimed_bytes = inner
            .reclaimed_bytes
            .saturating_add(u64::try_from(stats.freed_bytes).expect("usize fits u64"));
        inner.relocated_bytes = inner
            .relocated_bytes
            .saturating_add(u64::try_from(stats.relocated_bytes).expect("usize fits u64"));
    }

    pub fn record_execution_stats(&self, collector: &str, stats: &GcExecutionStats) {
        let mut inner = self.inner.lock().expect("GC telemetry lock poisoned");
        if inner.collector.is_empty() {
            inner.collector.push_str(collector);
        } else {
            assert_eq!(inner.collector, collector, "telemetry collector changed");
        }
        inner.cycles +=
            u64::try_from(stats.pause_hist.len()).expect("pause history length fits u64");
        for &pause_ns in &stats.pause_hist {
            inner.pause.record(pause_ns);
        }
        inner.reclaimed_bytes = inner
            .reclaimed_bytes
            .saturating_add(u64::try_from(stats.last.freed_bytes).expect("usize fits u64"));
        inner.relocated_bytes = inner
            .relocated_bytes
            .saturating_add(u64::try_from(stats.last.relocated_bytes).expect("usize fits u64"));
    }

    pub fn from_execution_stats(collector: &str, stats: &GcExecutionStats) -> Self {
        let telemetry = Self::default();
        telemetry.record_execution_stats(collector, stats);
        telemetry
    }

    pub fn snapshot(&self) -> GcTelemetrySnapshot {
        let inner = self.inner.lock().expect("GC telemetry lock poisoned");
        GcTelemetrySnapshot {
            schema_version: GC_TELEMETRY_SCHEMA_VERSION,
            collector: inner.collector.clone(),
            cycles: inner.cycles,
            pause: inner.pause.snapshot(),
            reclaimed_bytes: inner.reclaimed_bytes,
            relocated_bytes: inner.relocated_bytes,
            ..GcTelemetrySnapshot::default()
        }
    }

    pub fn to_json(&self) -> anyhow::Result<String> {
        json::to_json(&self.snapshot())
    }
}
