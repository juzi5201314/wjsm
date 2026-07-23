use anyhow::Result;
use std::time::Duration;

use crate::cli::RunArgs;
use crate::resource::HostInfo;
use crate::scenario::Scenario;
use crate::schema::*;
use crate::stats::summarize;
use crate::wjsm_driver::WjsmDriver;

pub(crate) fn run_bench(args: &RunArgs) -> Result<BenchReport> {
    let scenario = Scenario::build(
        args.scenario,
        args.seed,
        args.common.heap,
        args.live_set,
        args.objects,
    );
    let hardware = HostInfo::detect();
    let driver = WjsmDriver::compile(&scenario)?;
    let duration = Duration::from_secs(args.duration);

    let mut samples = Vec::with_capacity(args.samples);
    for index in 0..args.samples {
        let sample = driver.run_sample(args.gc, args.common.heap, duration)?;
        samples.push(SampleReport {
            index,
            steady_state_ns: sample.steady_state_ns,
            telemetry: sample.telemetry,
        });
    }

    let summary = build_summary(&samples);
    Ok(BenchReport {
        schema_version: BENCHMARK_SCHEMA_VERSION,
        config: BenchConfig {
            gc: args.gc.as_str().into(),
            heap_bytes: args.common.heap,
            scenario: scenario.name.into(),
            live_set_percent: args.live_set,
            samples: args.samples,
            duration_seconds: args.duration,
            seed: args.seed,
            allocations: scenario.allocations,
            retained: scenario.retained,
        },
        hardware,
        samples,
        summary,
    })
}

fn build_summary(samples: &[SampleReport]) -> BenchSummary {
    let steady: Vec<u64> = samples.iter().map(|s| s.steady_state_ns).collect();
    let gc_cpu: Vec<u64> = samples
        .iter()
        .map(|s| s.telemetry.gc_cpu_ns.unwrap_or(0))
        .collect();
    let pause_max: Vec<u64> = samples
        .iter()
        .map(|s| s.telemetry.pause.max_ns)
        .collect();

    let totals = aggregate_telemetry(samples);
    let metrics = derive_metrics(samples, &totals);

    BenchSummary {
        steady_state_ns: to_distribution(&summarize(&steady)),
        gc_cpu_ns: to_distribution(&summarize(&gc_cpu)),
        pause_max_ns: to_distribution(&summarize(&pause_max)),
        metrics,
        telemetry_totals: totals,
    }
}

/// 将所有样本的 telemetry 累计值求和。
fn aggregate_telemetry(samples: &[SampleReport]) -> wjsm_runtime::GcTelemetrySnapshot {
    let mut agg = wjsm_runtime::GcTelemetrySnapshot::default();
    for sample in samples {
        let t = &sample.telemetry;
        agg.cycles = agg.cycles.saturating_add(t.cycles);
        agg.reclaimed_bytes = agg.reclaimed_bytes.saturating_add(t.reclaimed_bytes);
        agg.relocated_bytes = agg.relocated_bytes.saturating_add(t.relocated_bytes);
        agg.gc_cpu_ns = add_opt(agg.gc_cpu_ns, t.gc_cpu_ns);
        agg.mark_cpu_ns = add_opt(agg.mark_cpu_ns, t.mark_cpu_ns);
        agg.relocation_cpu_ns = add_opt(agg.relocation_cpu_ns, t.relocation_cpu_ns);
        agg.physical_allocated_bytes =
            add_opt(agg.physical_allocated_bytes, t.physical_allocated_bytes);
        agg.mark_live_bytes = add_opt(agg.mark_live_bytes, t.mark_live_bytes);
        agg.barrier_load_fast_events =
            add_opt(agg.barrier_load_fast_events, t.barrier_load_fast_events);
        agg.barrier_store_fast_events =
            add_opt(agg.barrier_store_fast_events, t.barrier_store_fast_events);
        // pause 取所有样本中的最大值
        agg.pause.max_ns = agg.pause.max_ns.max(t.pause.max_ns);
        agg.pause.count = agg.pause.count.saturating_add(t.pause.count);
        if agg.collector.is_empty() {
            agg.collector = t.collector.clone();
        }
        agg.schema_version = t.schema_version;
    }
    agg
}

fn add_opt(acc: Option<u64>, val: Option<u64>) -> Option<u64> {
    match (acc, val) {
        (Some(a), Some(v)) => Some(a.saturating_add(v)),
        (Some(a), None) => Some(a),
        (None, Some(v)) => Some(v),
        (None, None) => None,
    }
}

fn derive_metrics(
    samples: &[SampleReport],
    totals: &wjsm_runtime::GcTelemetrySnapshot,
) -> DerivedMetrics {
    let total_steady_ns: u64 = samples.iter().map(|s| s.steady_state_ns).sum();
    let total_steady_sec = total_steady_ns as f64 / 1e9;

    let gc_cpu = totals.gc_cpu_ns;
    let allocated = totals.physical_allocated_bytes;
    let mark_cpu = totals.mark_cpu_ns;
    let mark_live = totals.mark_live_bytes;
    let reloc_cpu = totals.relocation_cpu_ns;
    let relocated = (totals.relocated_bytes > 0).then_some(totals.relocated_bytes);

    DerivedMetrics {
        gc_cpu_per_allocated_byte: ratio_f64(gc_cpu, allocated),
        mark_cpu_per_live_byte: ratio_f64(mark_cpu, mark_live),
        relocation_cpu_per_relocated_byte: ratio_f64(reloc_cpu, relocated),
        allocation_rate_bytes_per_sec: allocated
            .filter(|&a| a > 0 && total_steady_sec > 0.0)
            .map(|a| a as f64 / total_steady_sec),
        gc_overhead_percent: gc_cpu
            .filter(|&c| c > 0 && total_steady_ns > 0)
            .map(|c| c as f64 / total_steady_ns as f64 * 100.0),
        barrier_load_events_per_sec: totals
            .barrier_load_fast_events
            .filter(|&e| e > 0 && total_steady_sec > 0.0)
            .map(|e| e as f64 / total_steady_sec),
        barrier_store_events_per_sec: totals
            .barrier_store_fast_events
            .filter(|&e| e > 0 && total_steady_sec > 0.0)
            .map(|e| e as f64 / total_steady_sec),
        gc_cycles_per_sec: (totals.cycles > 0 && total_steady_sec > 0.0)
            .then(|| totals.cycles as f64 / total_steady_sec),
    }
}

fn ratio_f64(numerator: Option<u64>, denominator: Option<u64>) -> Option<f64> {
    numerator.zip(denominator).and_then(|(n, d)| {
        (d > 0).then_some(n as f64 / d as f64)
    })
}

fn to_distribution(s: &crate::stats::DistributionSummary) -> Distribution {
    Distribution {
        count: s.count,
        mean: s.mean,
        min: s.min,
        p50: s.p50,
        p99: s.p99,
        max: s.max,
    }
}
