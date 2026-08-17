use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cli::GcKind;
use crate::scenario::Scenario;

pub struct WjsmDriver {
    artifact: wjsm_artifact_format::PortableArtifact,
    runtime: wjsm_host_native::NativeRuntime,
}

pub struct WjsmSample {
    pub steady_state_ns: u64,
    pub telemetry: wjsm_gc::GcTelemetrySnapshot,
}

impl WjsmDriver {
    pub fn compile(scenario: &Scenario, gc: GcKind) -> Result<Self> {
        let source: Arc<str> = scenario.source.clone().into();
        let ast = wjsm_parser::parse_script_as_module(&source)?;
        let program = wjsm_semantic::lower_module_with_source(
            ast,
            true,
            Some(Arc::clone(&source)),
            "gc-bench.js",
        )
        .map_err(|error| anyhow::anyhow!("lower benchmark source: {error}"))?;
        let artifact = wjsm_artifact_format::PortableArtifact::from_input(
            &wjsm_artifact_format::ArtifactBuildInput {
                program: Arc::new(program),
                manifest: Arc::new(wjsm_artifact_format::ModuleManifest::single(
                    "gc-bench.js",
                    true,
                )),
                options: wjsm_artifact_format::BuildOptions::default(),
                source_text: None,
            },
        )
        .map_err(|error| anyhow::anyhow!("encode benchmark artifact: {error}"))?;
        let gc_algorithm = match gc {
            GcKind::Zgc => wjsm_gc::GcAlgorithmKind::Zgc,
            GcKind::G1 => wjsm_gc::GcAlgorithmKind::G1,
            GcKind::MarkSweep => wjsm_gc::GcAlgorithmKind::MarkSweep,
        };
        let config = wjsm_host_native::NativeRuntimeConfig::default()
            .with_gc_algorithm(gc_algorithm)
            .with_max_heap_size(scenario.heap_cap_bytes);
        let runtime = wjsm_host_native::NativeRuntime::new_with_config(config)?;
        Ok(Self { artifact, runtime })
    }

    pub fn run_sample(
        &mut self,
        _gc: GcKind,
        _heap_cap_bytes: u64,
        duration: Duration,
    ) -> Result<WjsmSample> {
        self.runtime.reset_gc_telemetry();
        let started = Instant::now();
        let mut steady_state_ns = 0_u64;
        loop {
            let execution_started = Instant::now();
            self.runtime.execute(
                &self.artifact,
                std::path::Path::new("."),
                std::path::Path::new("."),
            )?;
            steady_state_ns =
                steady_state_ns.saturating_add(execution_started.elapsed().as_nanos() as u64);
            if duration.is_zero() || started.elapsed() >= duration {
                break;
            }
        }
        Ok(WjsmSample {
            steady_state_ns,
            telemetry: self.runtime.gc_telemetry(),
        })
    }
}
