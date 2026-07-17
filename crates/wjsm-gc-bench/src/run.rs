use anyhow::{Context, Result};
use std::time::Duration;

use crate::EXIT_NEEDS_RESOURCE_RUNNER;
use crate::cli::{EngineKind, GcKind, RunArgs};
use crate::jdk_probe::{JdkProbeMetadata, inspect_jdk};
use crate::jvm_driver::{JdkCounters, JdkDriver};
use crate::report::write_json;
use crate::resource::{
    AdmissionDecision, AdmissionRequest, AdmissionStatus, HostResourceProvider,
    HostResourceSnapshot, SystemHostResourceProvider, admit,
};
use crate::scenario::{Scenario, ScenarioSpec};
use crate::schema::{
    BENCHMARK_SCHEMA_VERSION, BenchmarkManifest, CounterSource, GateStatus, HardwareMetadata,
    MetricObservation, RunConfiguration, RunReport, RuntimeMetadata, SampleReport,
};
use crate::stats::summarize;
use crate::wjsm_driver::WjsmDriver;

pub(crate) fn run_command(args: RunArgs) -> Result<i32> {
    let report = build_run_report(&args)?;
    write_json(&args.common.output, &report)?;
    if let Some(path) = &args.manifest {
        write_json(
            path,
            &BenchmarkManifest {
                schema_version: BENCHMARK_SCHEMA_VERSION,
                status: report.status.clone(),
                reports: vec![report.clone()],
                notes: report.notes.clone(),
            },
        )?;
    }
    Ok(exit_for_admission(&report.admission))
}

pub(crate) fn build_run_report(args: &RunArgs) -> Result<RunReport> {
    build_run_report_with_jdk(args, None)
}

pub(crate) fn build_run_report_with_jdk(
    args: &RunArgs,
    prepared_jdk: Option<&JdkProbeMetadata>,
) -> Result<RunReport> {
    if args.samples == 0 {
        anyhow::bail!("--samples must be at least one");
    }
    if args.live_set > 100 {
        anyhow::bail!("--live-set must be within 0..=100");
    }
    let resources = SystemHostResourceProvider.snapshot()?;
    let admission = admit(
        &resources,
        AdmissionRequest {
            heap_cap_bytes: args.common.heap,
            profile: args.common.profile,
        },
    );
    let scenario =
        ScenarioSpec::new(args.scenario, args.seed, args.common.heap, args.live_set).build();
    let configuration = configuration(args);
    if admission.status == AdmissionStatus::NeedsResourceRunner {
        return Ok(empty_run_report(
            args,
            scenario,
            resources,
            admission,
            configuration,
            GateStatus::NeedsResourceRunner,
            None,
            None,
            vec!["resource admission rejected before compile, instantiate, or child spawn".into()],
        ));
    }

    let unsupported_controls = unsupported_control_reasons(args);
    if !unsupported_controls.is_empty() {
        return Ok(empty_run_report(
            args,
            scenario,
            resources,
            admission,
            configuration,
            GateStatus::NeedsVerification,
            None,
            None,
            unsupported_controls,
        ));
    }

    match args.engine {
        EngineKind::Wjsm => build_wjsm_report(args, scenario, resources, admission, configuration),
        EngineKind::Jdk => {
            let metadata = match prepared_jdk {
                Some(metadata) => Ok(metadata.clone()),
                None => load_prepared_jdk(args),
            };
            match metadata {
                Ok(metadata) if metadata.classes_dir.is_dir() => build_jdk_report(
                    args,
                    scenario,
                    resources,
                    admission,
                    configuration,
                    metadata,
                ),
                Ok(metadata) => Ok(empty_run_report(
                    args,
                    scenario,
                    resources,
                    admission,
                    configuration,
                    GateStatus::NeedsVerification,
                    Some(metadata.patch_sha256),
                    Some("jdk-25-diagnostic-probe"),
                    vec![format!(
                        "JDK probe classes 不存在于 {}；先运行 prepare-jdk",
                        metadata.classes_dir.display()
                    )],
                )),
                Err(error) => Ok(empty_run_report(
                    args,
                    scenario,
                    resources,
                    admission,
                    configuration,
                    GateStatus::NeedsVerification,
                    None,
                    None,
                    vec![format!("JDK 25 环境不可验证：{error:#}")],
                )),
            }
        }
    }
}

fn build_wjsm_report(
    args: &RunArgs,
    scenario: Scenario,
    resources: HostResourceSnapshot,
    admission: AdmissionDecision,
    configuration: RunConfiguration,
) -> Result<RunReport> {
    let driver = WjsmDriver::compile(&scenario)?;
    let duration = Duration::from_secs(args.duration);
    let mut samples = Vec::with_capacity(args.samples);
    for index in 0..args.samples {
        let sample = driver.run_sample(args.gc, args.common.heap, duration)?;
        samples.push(SampleReport {
            index,
            steady_state_ns: sample.steady_state_ns,
            gc_telemetry: sample.telemetry,
            metrics: sample.metrics,
        });
    }
    Ok(build_report(
        args,
        scenario,
        resources,
        admission,
        configuration,
        samples,
        None,
        Some("wjsm-runtime-telemetry"),
    ))
}

fn build_jdk_report(
    args: &RunArgs,
    scenario: Scenario,
    resources: HostResourceSnapshot,
    admission: AdmissionDecision,
    configuration: RunConfiguration,
    metadata: JdkProbeMetadata,
) -> Result<RunReport> {
    if !metadata.classes_dir.is_dir() {
        return Ok(empty_run_report(
            args,
            scenario,
            resources,
            admission,
            configuration,
            GateStatus::NeedsVerification,
            Some(metadata.patch_sha256),
            Some("jdk-25-diagnostic-probe"),
            vec![format!(
                "JDK probe classes 不存在于 {}；先运行 prepare-jdk",
                metadata.classes_dir.display()
            )],
        ));
    }
    if args.gc == GcKind::MarkSweep {
        return Ok(empty_run_report(
            args,
            scenario,
            resources,
            admission,
            configuration,
            GateStatus::NeedsVerification,
            Some(metadata.patch_sha256),
            Some("jdk-25-stock"),
            vec!["JDK 没有与 WJSM mark-sweep 对应的 collector；需要 JDK reference runner".into()],
        ));
    }
    let driver = JdkDriver::new(metadata.clone());
    let mut samples = Vec::with_capacity(args.samples);
    for index in 0..args.samples {
        let sample = driver.run_sample(&scenario, args.gc)?;
        samples.push(SampleReport {
            index,
            steady_state_ns: sample.steady_state_ns,
            gc_telemetry: jdk_telemetry(args.gc),
            metrics: jdk_metrics(sample.counters),
        });
    }
    let mut report = build_report(
        args,
        scenario,
        resources,
        admission,
        configuration,
        samples,
        Some(metadata.patch_sha256),
        Some(
            metadata
                .diagnostic_counters_available
                .then_some("jdk-25-diagnostic-probe")
                .unwrap_or("jdk-25-stock"),
        ),
    );
    if !metadata.diagnostic_counters_available {
        report.status = GateStatus::NeedsVerification;
        report
            .notes
            .push("stock JDK 未提供所需内部 numerator；不能通过 normalized gate".into());
    }
    Ok(report)
}

fn build_report(
    args: &RunArgs,
    scenario: Scenario,
    resources: HostResourceSnapshot,
    admission: AdmissionDecision,
    configuration: RunConfiguration,
    samples: Vec<SampleReport>,
    patch_sha256: Option<String>,
    counter_source: Option<&str>,
) -> RunReport {
    let steady_samples: Vec<_> = samples
        .iter()
        .map(|sample| sample.steady_state_ns)
        .collect();
    let status = if samples.iter().all(has_complete_internal_metrics) {
        GateStatus::Passed
    } else {
        GateStatus::NeedsVerification
    };
    RunReport {
        schema_version: BENCHMARK_SCHEMA_VERSION,
        status,
        runtime: runtime_metadata(args, patch_sha256, counter_source),
        scenario: scenario.manifest,
        denominators: scenario.denominators,
        resources,
        admission,
        configuration,
        steady_state: summarize(&steady_samples),
        samples,
        notes: vec![
            "steady_state_ns excludes JS/Wasm compilation, Wasmtime compilation, instantiate, and startup".into(),
            "missing physical allocation, CPU, barrier, or JDK counters force needs-verification".into(),
        ],
    }
}

#[allow(clippy::too_many_arguments)]
fn empty_run_report(
    args: &RunArgs,
    scenario: Scenario,
    resources: HostResourceSnapshot,
    admission: AdmissionDecision,
    configuration: RunConfiguration,
    status: GateStatus,
    patch_sha256: Option<String>,
    counter_source: Option<&str>,
    notes: Vec<String>,
) -> RunReport {
    RunReport {
        schema_version: BENCHMARK_SCHEMA_VERSION,
        status,
        runtime: runtime_metadata(args, patch_sha256, counter_source),
        scenario: scenario.manifest,
        denominators: scenario.denominators,
        resources,
        admission,
        configuration,
        samples: Vec::new(),
        steady_state: Default::default(),
        notes,
    }
}

fn load_prepared_jdk(args: &RunArgs) -> Result<JdkProbeMetadata> {
    let jdk_home = args
        .jdk_home
        .as_deref()
        .context("--jdk-home is required for --engine jdk")?;
    let probe_home = args
        .jdk_probe_home
        .as_deref()
        .context("--jdk-probe-home is required for --engine jdk")?;
    inspect_jdk(jdk_home, probe_home)
}

fn configuration(args: &RunArgs) -> RunConfiguration {
    RunConfiguration {
        profile: args.common.profile,
        samples: args.samples,
        duration_seconds: args.duration,
        workers: args.workers,
        relocate_every_page: args.relocate_every_page,
        barrier_buffer_capacity: args.barrier_buffer_capacity,
        safepoint_every_allocation: args.safepoint_every_allocation,
        jdk_home: args.jdk_home.clone(),
        jdk_probe_home: args.jdk_probe_home.clone(),
    }
}

fn unsupported_control_reasons(args: &RunArgs) -> Vec<String> {
    let mut reasons = Vec::new();
    if args.workers != 1 {
        reasons.push("--workers 尚未接入当前单线程 collector runtime".into());
    }
    if args.relocate_every_page {
        reasons.push("--relocate-every-page 在 Task 20 前不可用".into());
    }
    if args.barrier_buffer_capacity != 4096 {
        reasons.push("--barrier-buffer-capacity 在 Task 16 前不可用".into());
    }
    if args.safepoint_every_allocation {
        reasons.push("--safepoint-every-allocation 在 Task 16 前不可用".into());
    }
    reasons
}

fn runtime_metadata(
    args: &RunArgs,
    patch_sha256: Option<String>,
    counter_source: Option<&str>,
) -> RuntimeMetadata {
    RuntimeMetadata {
        engine: args.engine.as_str().into(),
        gc: args.gc.as_str().into(),
        tool_version: env!("CARGO_PKG_VERSION").into(),
        wasmtime_version: wjsm_engine_config::WASMTIME_VERSION.into(),
        hardware: hardware_metadata(),
        jdk_probe_patch_sha256: patch_sha256,
        counter_source: counter_source.map(str::to_owned),
    }
}

pub(crate) fn hardware_metadata() -> HardwareMetadata {
    HardwareMetadata {
        architecture: std::env::consts::ARCH.into(),
        os: std::env::consts::OS.into(),
        logical_cpus: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
    }
}

fn jdk_telemetry(gc: GcKind) -> wjsm_runtime::GcTelemetrySnapshot {
    wjsm_runtime::GcTelemetrySnapshot {
        schema_version: wjsm_runtime::GC_TELEMETRY_SCHEMA_VERSION,
        collector: gc.as_str().into(),
        ..Default::default()
    }
}

fn jdk_metrics(counters: Option<JdkCounters>) -> Vec<MetricObservation> {
    let metrics = counters.map(|counters| {
        [
            (
                "gc_cpu_per_allocated_byte",
                counters.gc_cpu_ns,
                counters.physical_allocated_bytes,
            ),
            ("mark_cpu_per_live_byte", counters.mark_cpu_ns, None),
            (
                "relocation_cpu_per_relocated_byte",
                counters.relocation_cpu_ns,
                counters.relocated_bytes,
            ),
        ]
    });
    metric_names()
        .into_iter()
        .map(|name| {
            let pair = metrics.as_ref().and_then(|values| {
                values
                    .iter()
                    .find(|(candidate, _, _)| *candidate == name)
                    .map(|(_, numerator, denominator)| (*numerator, *denominator))
            });
            let (numerator, denominator) = pair.unwrap_or((None, None));
            benchmark_metric(name, numerator, denominator, "jdk-25-diagnostic-probe")
        })
        .collect()
}

pub(crate) fn metric_names() -> [&'static str; 5] {
    [
        "gc_cpu_per_allocated_byte",
        "mark_cpu_per_live_byte",
        "relocation_cpu_per_relocated_byte",
        "barrier_load_retired_instructions_per_event",
        "barrier_store_retired_instructions_per_event",
    ]
}

fn benchmark_metric(
    name: &str,
    numerator: Option<u64>,
    denominator: Option<u64>,
    source: &str,
) -> MetricObservation {
    let numerator = numerator.map(|value| value as f64);
    let denominator = denominator.map(|value| value as f64);
    MetricObservation {
        name: name.into(),
        value: numerator.zip(denominator).map(|(top, bottom)| top / bottom),
        numerator,
        denominator,
        source: CounterSource {
            name: source.into(),
            detail: "missing counters remain null and require verification".into(),
        },
    }
}

fn has_complete_internal_metrics(sample: &SampleReport) -> bool {
    sample.metrics.iter().all(|metric| metric.value.is_some())
}

pub(crate) fn exit_for_admission(admission: &AdmissionDecision) -> i32 {
    match admission.status {
        AdmissionStatus::Admitted => 0,
        AdmissionStatus::NeedsResourceRunner => EXIT_NEEDS_RESOURCE_RUNNER,
    }
}
