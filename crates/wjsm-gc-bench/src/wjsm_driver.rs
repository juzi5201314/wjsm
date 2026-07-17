use anyhow::Result;
use std::time::{Duration, Instant};

use crate::cli::GcKind;
use crate::scenario::Scenario;
use crate::schema::{CounterSource, MetricObservation};

pub struct WjsmDriver {
    wasm: Vec<u8>,
    runtime: tokio::runtime::Runtime,
}

pub struct WjsmSample {
    pub steady_state_ns: u64,
    pub telemetry: wjsm_runtime::GcTelemetrySnapshot,
    pub metrics: Vec<MetricObservation>,
}

impl WjsmDriver {
    pub fn compile(scenario: &Scenario) -> Result<Self> {
        Ok(Self {
            wasm: wjsm_runtime::compile_source(&scenario.source)?,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .map_err(|error| anyhow::anyhow!("create WJSM benchmark runtime: {error}"))?,
        })
    }

    pub fn run_sample(
        &self,
        gc: GcKind,
        heap_cap_bytes: u64,
        duration: Duration,
    ) -> Result<WjsmSample> {
        let options = wjsm_runtime::RuntimeOptions {
            max_heap_size: Some(
                usize::try_from(heap_cap_bytes)
                    .map_err(|_| anyhow::anyhow!("heap cap exceeds usize"))?,
            ),
            gc_algorithm: gc_algorithm(gc),
            ..wjsm_runtime::RuntimeOptions::default()
        };
        let telemetry = wjsm_runtime::GcTelemetry::default();
        let started = Instant::now();
        let mut steady_state_ns = 0_u64;
        loop {
            let execution =
                self.runtime
                    .block_on(wjsm_runtime::execute_wasm_steady_state_for_bench(
                        &self.wasm,
                        options.clone(),
                    ))?;
            steady_state_ns = steady_state_ns.saturating_add(execution.gc_stats.steady_state_ns);
            telemetry.record_execution_stats(gc.as_str(), &execution.gc_stats);
            if duration.is_zero() || started.elapsed() >= duration {
                break;
            }
        }
        let telemetry = telemetry.snapshot();
        Ok(WjsmSample {
            steady_state_ns,
            metrics: metric_observations(&telemetry),
            telemetry,
        })
    }
}

fn gc_algorithm(gc: GcKind) -> wjsm_runtime::GcAlgorithmKind {
    match gc {
        GcKind::Zgc => wjsm_runtime::GcAlgorithmKind::Zgc,
        GcKind::G1 => wjsm_runtime::GcAlgorithmKind::G1,
        GcKind::MarkSweep => wjsm_runtime::GcAlgorithmKind::MarkSweep,
    }
}

fn metric_observations(telemetry: &wjsm_runtime::GcTelemetrySnapshot) -> Vec<MetricObservation> {
    [
        metric(
            "gc_cpu_per_allocated_byte",
            telemetry.gc_cpu_ns,
            telemetry.physical_allocated_bytes,
        ),
        metric("mark_cpu_per_live_byte", telemetry.mark_cpu_ns, None),
        metric(
            "relocation_cpu_per_relocated_byte",
            telemetry.relocation_cpu_ns,
            (telemetry.relocated_bytes > 0).then_some(telemetry.relocated_bytes),
        ),
        metric(
            "barrier_load_retired_instructions_per_event",
            None,
            telemetry.barrier_load_fast_events,
        ),
        metric(
            "barrier_store_retired_instructions_per_event",
            None,
            telemetry.barrier_store_fast_events,
        ),
    ]
    .into()
}

fn metric(name: &str, numerator: Option<u64>, denominator: Option<u64>) -> MetricObservation {
    let numerator = numerator.map(|value| value as f64);
    let denominator = denominator.map(|value| value as f64);
    MetricObservation {
        name: name.into(),
        value: numerator.zip(denominator).map(|(top, bottom)| top / bottom),
        numerator,
        denominator,
        source: CounterSource {
            name: "wjsm-runtime-telemetry".into(),
            detail: "missing counters remain null and require verification".into(),
        },
    }
}
