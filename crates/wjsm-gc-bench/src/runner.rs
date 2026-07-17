use anyhow::Result;
use clap::ValueEnum;
use serde::Serialize;
use std::time::Instant;

use wjsm_runtime::{ManagedAllocator, ManagedHeapLayout, Nlab};

use crate::cli::{Cli, Command, CommonArgs, JdkArgs, MicroArgs, ReplayArgs, RunArgs};
use crate::comparison::{compare, gate};
use crate::jdk_probe::prepare_jdk;
use crate::report::{read_json, write_json};
use crate::resource::{
    AdmissionDecision, AdmissionRequest, AdmissionStatus, HostResourceProvider,
    HostResourceSnapshot, SystemHostResourceProvider, admit,
};
use crate::run::{build_run_report, exit_for_admission, hardware_metadata, run_command};
use crate::scenario::ScenarioKind;
use crate::schema::{BENCHMARK_SCHEMA_VERSION, BenchmarkManifest, GateStatus, HardwareMetadata};

pub fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Capabilities(args) => capabilities(args),
        Command::Preflight(args) => preflight(args),
        Command::PrepareJdk(args) => prepare_jdk_command(args),
        Command::Baseline(args) | Command::Run(args) => run_command(args),
        Command::Micro(args) => micro(args),
        Command::Compare(args) => compare(args),
        Command::Replay(args) => replay(args),
        Command::Gate(args) => gate(args),
    }
}

fn capabilities(args: CommonArgs) -> Result<i32> {
    let resources = SystemHostResourceProvider.snapshot()?;
    write_json(
        &args.output,
        &CapabilityReport {
            schema_version: BENCHMARK_SCHEMA_VERSION,
            hardware: hardware_metadata(),
            resources,
        },
    )?;
    Ok(0)
}

fn preflight(args: CommonArgs) -> Result<i32> {
    let resources = SystemHostResourceProvider.snapshot()?;
    let admission = admit(
        &resources,
        AdmissionRequest {
            heap_cap_bytes: args.heap,
            profile: args.profile,
        },
    );
    write_json(
        &args.output,
        &PreflightReport {
            schema_version: BENCHMARK_SCHEMA_VERSION,
            resources,
            admission: admission.clone(),
        },
    )?;
    Ok(exit_for_admission(&admission))
}

fn prepare_jdk_command(args: JdkArgs) -> Result<i32> {
    let report = match (args.jdk_home.as_deref(), args.jdk_probe_home.as_deref()) {
        (Some(jdk_home), Some(probe_home)) => match prepare_jdk(jdk_home, probe_home) {
            Ok(metadata) => PrepareJdkReport {
                schema_version: BENCHMARK_SCHEMA_VERSION,
                status: GateStatus::Passed,
                metadata: Some(metadata),
                reason: None,
            },
            Err(error) => PrepareJdkReport {
                schema_version: BENCHMARK_SCHEMA_VERSION,
                status: GateStatus::NeedsVerification,
                metadata: None,
                reason: Some(format!("JDK 25 probe 未就绪：{error:#}")),
            },
        },
        _ => PrepareJdkReport {
            schema_version: BENCHMARK_SCHEMA_VERSION,
            status: GateStatus::NeedsVerification,
            metadata: None,
            reason: Some("需要 --jdk-home 与 --jdk-probe-home 才能准备 JDK 25".into()),
        },
    };
    write_json(&args.common.output, &report)?;
    Ok(0)
}

fn micro(args: MicroArgs) -> Result<i32> {
    let resources = SystemHostResourceProvider.snapshot()?;
    let admission = admit(
        &resources,
        AdmissionRequest {
            heap_cap_bytes: args.common.heap,
            profile: args.common.profile,
        },
    );
    let (status, measurements, reason) = match admission.status {
        AdmissionStatus::NeedsResourceRunner => (
            GateStatus::NeedsResourceRunner,
            Vec::new(),
            Some("host resource admission rejected allocator micro run".into()),
        ),
        AdmissionStatus::Admitted => match args.component.as_str() {
            "allocator" => (
                GateStatus::Passed,
                run_allocator_micro(args.common.heap, args.samples)?,
                None,
            ),
            component => anyhow::bail!("unknown micro component `{component}`"),
        },
    };
    let report = MicroReport {
        schema_version: BENCHMARK_SCHEMA_VERSION,
        status,
        component: args.component,
        samples: args.samples,
        resources,
        admission: admission.clone(),
        measurements,
        reason,
    };
    write_json(&args.common.output, &report)?;
    Ok(exit_for_admission(&admission))
}

fn run_allocator_micro(heap: u64, samples: usize) -> Result<Vec<AllocatorMicroSample>> {
    const CONTROL_RESERVED: u64 = 64 * 1024;
    const PAGE_BYTES: u64 = 64 * 1024;
    let operations = (heap / PAGE_BYTES).clamp(4096, 65_536);
    let mut measurements = Vec::with_capacity(samples);

    for sample in 0..samples {
        let layout = ManagedHeapLayout::new(heap, CONTROL_RESERVED)?;
        let allocator = ManagedAllocator::new(layout)?;
        let mut nlab = Nlab::new();
        let start = Instant::now();
        let mut expected_bytes = 0;
        for index in 0..operations {
            let bytes = 8 + ((index + sample as u64) % 31) * 8;
            allocator.allocate(&mut nlab, bytes)?;
            expected_bytes += bytes;
        }
        let allocated_bytes = allocator.allocated_bytes();
        anyhow::ensure!(
            allocated_bytes == expected_bytes,
            "allocator byte counter diverged: expected {expected_bytes}, got {allocated_bytes}"
        );
        measurements.push(AllocatorMicroSample {
            elapsed_ns: start.elapsed().as_nanos().try_into().unwrap_or(u64::MAX),
            allocated_objects: operations,
            allocated_bytes,
            committed_bytes: allocator.committed_bytes(),
        });
    }
    Ok(measurements)
}

fn replay(args: ReplayArgs) -> Result<i32> {
    let manifest: BenchmarkManifest = read_json(&args.manifest)?;
    let mut replayed = BenchmarkManifest::empty();
    for report in manifest.reports {
        let engine = match report.runtime.engine.as_str() {
            "wjsm" => crate::cli::EngineKind::Wjsm,
            "jdk" => crate::cli::EngineKind::Jdk,
            unexpected => anyhow::bail!("cannot replay unknown engine `{unexpected}`"),
        };
        let gc = match report.runtime.gc.as_str() {
            "zgc" => crate::cli::GcKind::Zgc,
            "g1" => crate::cli::GcKind::G1,
            "mark-sweep" => crate::cli::GcKind::MarkSweep,
            unexpected => anyhow::bail!("cannot replay unknown collector `{unexpected}`"),
        };
        let scenario = ScenarioKind::from_str(&report.scenario.name, true)
            .map_err(|error| anyhow::anyhow!("invalid replay scenario: {error}"))?;
        let run = RunArgs {
            common: CommonArgs {
                heap: report.scenario.heap_cap_bytes,
                output: args.output.clone(),
                profile: report.configuration.profile,
            },
            engine,
            gc,
            live_set: report.scenario.live_set_percent,
            scenario,
            samples: report.configuration.samples,
            duration: report.configuration.duration_seconds,
            workers: report.configuration.workers,
            seed: report.scenario.seed,
            manifest: None,
            jdk_home: report.configuration.jdk_home.clone(),
            jdk_probe_home: report.configuration.jdk_probe_home.clone(),
            relocate_every_page: report.configuration.relocate_every_page,
            barrier_buffer_capacity: report.configuration.barrier_buffer_capacity,
            safepoint_every_allocation: report.configuration.safepoint_every_allocation,
        };
        replayed.reports.push(build_run_report(&run)?);
    }
    write_json(&args.output, &replayed)?;
    Ok(0)
}

#[derive(Serialize)]
struct CapabilityReport {
    schema_version: u32,
    hardware: HardwareMetadata,
    resources: HostResourceSnapshot,
}

#[derive(Serialize)]
struct PreflightReport {
    schema_version: u32,
    resources: HostResourceSnapshot,
    admission: AdmissionDecision,
}

#[derive(Serialize)]
struct PrepareJdkReport {
    schema_version: u32,
    status: GateStatus,
    metadata: Option<crate::jdk_probe::JdkProbeMetadata>,
    reason: Option<String>,
}

#[derive(Serialize)]
struct MicroReport {
    schema_version: u32,
    status: GateStatus,
    component: String,
    samples: usize,
    resources: HostResourceSnapshot,
    admission: AdmissionDecision,
    measurements: Vec<AllocatorMicroSample>,
    reason: Option<String>,
}

#[derive(Serialize)]
struct AllocatorMicroSample {
    elapsed_ns: u64,
    allocated_objects: u64,
    allocated_bytes: u64,
    committed_bytes: u64,
}
