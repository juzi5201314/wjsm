use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::Serialize;

use crate::EXIT_NEEDS_RESOURCE_RUNNER;
use crate::cli::{CommonArgs, CompareArgs, EngineKind, GateArgs, GcKind, RunArgs};
use crate::gate::{GateReport, NormalizedMetric, PauseGateResult, evaluate_gate, evaluate_pause};
use crate::jdk_probe::prepare_jdk;
use crate::report::{read_json, write_json};
use crate::resource::{
    AdmissionDecision, AdmissionRequest, AdmissionStatus, HostResourceProvider,
    HostResourceSnapshot, SystemHostResourceProvider, admit,
};
use crate::run::{build_run_report, build_run_report_with_jdk, metric_names};
use crate::scenario::ScenarioKind;
use crate::schema::{BENCHMARK_SCHEMA_VERSION, BenchmarkManifest, GateStatus, RunReport};

pub(crate) fn compare(args: CompareArgs) -> Result<i32> {
    // Nightly: single `--heap` (common). PR: multi `--heaps`.
    let heaps = if args.common.profile == crate::cli::Profile::Nightly {
        vec![args.common.heap]
    } else {
        parse_csv(&args.heaps, crate::cli::parse_bytes)?
    };
    let largest_heap = *heaps
        .iter()
        .max()
        .context("--heaps/--heap must not be empty")?;
    let resources = SystemHostResourceProvider.snapshot()?;
    let admission = admit(
        &resources,
        AdmissionRequest {
            heap_cap_bytes: largest_heap,
            profile: args.common.profile,
        },
    );
    std::fs::create_dir_all(&args.common.output)
        .with_context(|| format!("create {}", args.common.output.display()))?;
    let manifest_path = args.common.output.join("manifest.json");
    if admission.status == AdmissionStatus::NeedsResourceRunner {
        let mut manifest = BenchmarkManifest::empty();
        manifest.status = GateStatus::NeedsResourceRunner;
        manifest
            .notes
            .push("resource admission rejected before javac/java child spawn".into());
        write_json(&manifest_path, &manifest)?;
        write_json(
            &args.common.output.join("preflight.json"),
            &ComparisonPreflightReport {
                schema_version: BENCHMARK_SCHEMA_VERSION,
                resources,
                admission,
            },
        )?;
        return Ok(EXIT_NEEDS_RESOURCE_RUNNER);
    }

    // 所有 resource admission 已通过，才允许 javac/java child spawn。
    let jdk_metadata = match prepare_jdk(&args.jdk_home, &args.jdk_probe_home) {
        Ok(metadata) => metadata,
        Err(error) => {
            let mut manifest = BenchmarkManifest::empty();
            manifest
                .notes
                .push(format!("JDK 25 环境不可验证：{error:#}"));
            write_json(&manifest_path, &manifest)?;
            return Ok(0);
        }
    };
    let live_sets = parse_csv(&args.live_sets, |value| {
        value
            .parse::<u8>()
            .map_err(|error| format!("invalid live set `{value}`: {error}"))
    })?;
    let scenarios = parse_csv(&args.scenarios, |value| {
        ScenarioKind::from_str(value, true).map_err(|error| error.to_string())
    })?;
    let mut manifest = BenchmarkManifest::empty();

    for heap in heaps {
        for live_set in &live_sets {
            for scenario in &scenarios {
                let common = CommonArgs {
                    heap,
                    output: args.common.output.join("unused.json"),
                    profile: args.common.profile,
                };
                let base = RunArgs {
                    common,
                    engine: EngineKind::Wjsm,
                    gc: GcKind::Zgc,
                    live_set: *live_set,
                    scenario: *scenario,
                    samples: args.samples,
                    duration: args.duration,
                    workers: 1,
                    seed: 0x5eed,
                    manifest: None,
                    jdk_home: Some(args.jdk_home.clone()),
                    jdk_probe_home: Some(args.jdk_probe_home.clone()),
                    relocate_every_page: false,
                    barrier_buffer_capacity: 4096,
                    safepoint_every_allocation: false,
                };
                manifest.reports.push(build_run_report(&base)?);
                let mut jdk = base;
                jdk.engine = EngineKind::Jdk;
                manifest
                    .reports
                    .push(build_run_report_with_jdk(&jdk, Some(&jdk_metadata))?);
            }
        }
    }
    let requires_runner = manifest
        .reports
        .iter()
        .any(|report| report.admission.status == AdmissionStatus::NeedsResourceRunner);
    manifest.status = if requires_runner {
        GateStatus::NeedsResourceRunner
    } else if manifest
        .reports
        .iter()
        .all(|report| report.status == GateStatus::Passed)
    {
        GateStatus::Passed
    } else {
        GateStatus::NeedsVerification
    };
    write_json(&manifest_path, &manifest)?;
    Ok(if requires_runner {
        EXIT_NEEDS_RESOURCE_RUNNER
    } else {
        0
    })
}

pub(crate) fn gate(args: GateArgs) -> Result<i32> {
    let manifest: BenchmarkManifest = read_json(&args.manifest)?;
    let report = gate_manifest_for_profile(&manifest, args.profile);
    let exit = match report.status {
        GateStatus::Passed => 0,
        GateStatus::NeedsResourceRunner => EXIT_NEEDS_RESOURCE_RUNNER,
        GateStatus::Failed | GateStatus::NeedsVerification => 1,
    };
    let output = args.manifest.with_extension("gate.json");
    write_json(&output, &report)?;
    Ok(exit)
}

pub fn gate_manifest(manifest: &BenchmarkManifest) -> GateReport {
    gate_manifest_for_profile(manifest, crate::cli::Profile::Pr)
}

pub fn gate_manifest_for_profile(
    manifest: &BenchmarkManifest,
    profile: crate::cli::Profile,
) -> GateReport {
    if manifest.status == GateStatus::NeedsResourceRunner {
        return GateReport {
            status: GateStatus::NeedsResourceRunner,
            metrics: Vec::new(),
            pauses: Vec::new(),
            leading_metrics: 0,
            reasons: manifest.notes.clone(),
        };
    }
    let mut metrics = Vec::new();
    let mut pauses = Vec::new();
    for wjsm in manifest
        .reports
        .iter()
        .filter(|report| report.runtime.engine == "wjsm")
    {
        let Some(jdk) = manifest.reports.iter().find(|report| {
            report.runtime.engine == "jdk"
                && report.scenario.logical_graph_hash == wjsm.scenario.logical_graph_hash
        }) else {
            for name in metric_names() {
                metrics.push(NormalizedMetric {
                    name: format!("{}:{name}", wjsm.scenario.logical_graph_hash),
                    wjsm_numerator: None,
                    wjsm_denominator: None,
                    jdk_numerator: None,
                    jdk_denominator: None,
                });
            }
            pauses.push(evaluate_pause(
                wjsm.scenario.logical_graph_hash.clone(),
                None,
                None,
                None,
            ));
            continue;
        };
        for name in metric_names() {
            metrics.push(paired_metric(name, wjsm, jdk));
        }
        pauses.push(paired_pause(wjsm, jdk));
    }
    if metrics.is_empty() {
        return GateReport {
            status: GateStatus::NeedsVerification,
            metrics: Vec::new(),
            pauses,
            leading_metrics: 0,
            reasons: vec!["manifest 未包含 WJSM/JDK 对比对".into()],
        };
    }
    let mut report = evaluate_gate(&metrics, &pauses);
    if profile == crate::cli::Profile::Nightly {
        apply_nightly_gates(manifest, &mut report);
    }
    report
}

fn apply_nightly_gates(manifest: &BenchmarkManifest, report: &mut GateReport) {
    let mut nightly_reasons = Vec::new();
    for run in &manifest.reports {
        if !run.resources.hard_isolation {
            nightly_reasons.push(format!(
                "nightly 需要 hard isolation；run {}/{} 缺少 delegated cgroup/Job",
                run.runtime.engine, run.scenario.name
            ));
        }
        if run.configuration.duration_seconds < 3600 {
            nightly_reasons.push(format!(
                "nightly duration={}s < 3600s for {}/{}",
                run.configuration.duration_seconds, run.runtime.engine, run.scenario.name
            ));
        }
        let child_ceiling = run
            .scenario
            .heap_cap_bytes
            .saturating_mul(2)
            .saturating_add(2 * 1024 * 1024 * 1024);
        let observed_limit = run
            .resources
            .cgroup_limit_bytes
            .or(run.resources.job_limit_bytes);
        if observed_limit.is_none_or(|limit| limit > child_ceiling) {
            nightly_reasons.push(format!(
                "nightly child hard ceiling 必须 <= 2*heap+2GiB (ceiling={child_ceiling}); run {}/{} 未证明",
                run.runtime.engine, run.scenario.name
            ));
        }
    }
    if !nightly_reasons.is_empty() {
        report.reasons.extend(nightly_reasons);
        if report.status == GateStatus::Passed {
            report.status = GateStatus::NeedsResourceRunner;
        }
    }
}

fn paired_metric(name: &str, wjsm: &RunReport, jdk: &RunReport) -> NormalizedMetric {
    let (wjsm_numerator, wjsm_denominator) =
        aggregate_metric_parts(wjsm, name).unwrap_or((None, None));
    let (jdk_numerator, jdk_denominator) =
        aggregate_metric_parts(jdk, name).unwrap_or((None, None));
    NormalizedMetric {
        name: format!("{}:{name}", wjsm.scenario.logical_graph_hash),
        wjsm_numerator,
        wjsm_denominator,
        jdk_numerator,
        jdk_denominator,
    }
}

fn aggregate_metric_parts(report: &RunReport, name: &str) -> Option<(Option<f64>, Option<f64>)> {
    if report.samples.is_empty() {
        return None;
    }
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for sample in &report.samples {
        let metric = sample.metrics.iter().find(|metric| metric.name == name)?;
        let (Some(top), Some(bottom)) = (metric.numerator, metric.denominator) else {
            return None;
        };
        if bottom <= 0.0 {
            return None;
        }
        numerator += top;
        denominator += bottom;
    }
    Some((Some(numerator), Some(denominator)))
}

fn paired_pause(wjsm: &RunReport, jdk: &RunReport) -> PauseGateResult {
    let wjsm_p999 = aggregate_pause(wjsm, |pause| pause.p999_ns);
    let jdk_p999 = aggregate_pause(jdk, |pause| pause.p999_ns);
    let wjsm_max = aggregate_pause(wjsm, |pause| pause.max_ns);
    evaluate_pause(
        wjsm.scenario.logical_graph_hash.clone(),
        wjsm_p999,
        jdk_p999,
        wjsm_max,
    )
}

fn aggregate_pause(
    report: &RunReport,
    select: impl Fn(&wjsm_runtime::HistogramSnapshot) -> u64,
) -> Option<u64> {
    if report.samples.is_empty()
        || report
            .samples
            .iter()
            .any(|sample| sample.gc_telemetry.pause.count == 0)
    {
        return None;
    }
    report
        .samples
        .iter()
        .map(|sample| select(&sample.gc_telemetry.pause))
        .max()
}

fn parse_csv<T>(
    input: &str,
    parse: impl Fn(&str) -> std::result::Result<T, String>,
) -> Result<Vec<T>> {
    input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| parse(value).map_err(anyhow::Error::msg))
        .collect()
}

#[derive(Serialize)]
struct ComparisonPreflightReport {
    schema_version: u32,
    resources: HostResourceSnapshot,
    admission: AdmissionDecision,
}
