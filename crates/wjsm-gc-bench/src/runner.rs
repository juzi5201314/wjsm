use anyhow::Result;
use clap::ValueEnum;
use serde::Serialize;

use crate::cli::{Cli, Command, CommonArgs, JdkArgs, MicroArgs, Profile, ReplayArgs, RunArgs};
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
    let report = MicroReport {
        schema_version: BENCHMARK_SCHEMA_VERSION,
        status: match admission.status {
            AdmissionStatus::Admitted => GateStatus::NeedsVerification,
            AdmissionStatus::NeedsResourceRunner => GateStatus::NeedsResourceRunner,
        },
        component: args.component,
        samples: args.samples,
        resources,
        admission: admission.clone(),
        reason: "micro component counters are unavailable until the managed allocator owner lands in Task 6".into(),
    };
    write_json(&args.common.output, &report)?;
    Ok(exit_for_admission(&admission))
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
                profile: Profile::Pr,
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
    reason: String,
}
