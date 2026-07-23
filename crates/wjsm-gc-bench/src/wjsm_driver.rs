use anyhow::Result;
use std::time::{Duration, Instant};

use crate::cli::GcKind;
use crate::scenario::Scenario;

pub struct WjsmDriver {
    wasm: Vec<u8>,
    runtime: tokio::runtime::Runtime,
}

pub struct WjsmSample {
    pub steady_state_ns: u64,
    pub telemetry: wjsm_runtime::GcTelemetrySnapshot,
}

impl WjsmDriver {
    pub fn compile(scenario: &Scenario) -> Result<Self> {
        Ok(Self {
            wasm: wjsm_runtime::compile_source(&scenario.source)?,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .map_err(|error| anyhow::anyhow!("创建 benchmark runtime: {error}"))?,
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
                    .map_err(|_| anyhow::anyhow!("heap cap 超出 usize"))?,
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
        Ok(WjsmSample {
            steady_state_ns,
            telemetry: telemetry.snapshot(),
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
