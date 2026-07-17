use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::Serialize;

use crate::EXIT_NEEDS_RESOURCE_RUNNER;
use crate::cli::{CommonArgs, CompareArgs, EngineKind, GateArgs, GcKind, RunArgs};
use crate::gate::{GateReport, NormalizedMetric, evaluate_gate};
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
    let heaps = parse_csv(&args.heaps, crate::cli::parse_bytes)?;
    let largest_heap = *heaps.iter().max().context("--heaps must not be empty")?;
    let resources = SystemHostResourceProvider.snapshot()?;
    let admission = admit(
        &resources,
        AdmissionRequest {
            heap_cap_bytes: largest_heap,
            profile: args.common.profile,
        },
    );
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
                    duration: 0,
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
    Ok(requires_runner
        .then_some(EXIT_NEEDS_RESOURCE_RUNNER)
        .unwrap_or(0))
}

pub(crate) fn gate(args: GateArgs) -> Result<i32> {
    let manifest: BenchmarkManifest = read_json(&args.manifest)?;
    let report = gate_manifest(&manifest);
    let exit = match report.status {
        GateStatus::Passed => 0,
        GateStatus::NeedsResourceRunner => EXIT_NEEDS_RESOURCE_RUNNER,
        GateStatus::Failed | GateStatus::NeedsVerification => 1,
    };
    let output = args.manifest.with_extension("gate.json");
    write_json(&output, &report)?;
    Ok(exit)
}

pub(crate) fn gate_manifest(manifest: &BenchmarkManifest) -> GateReport {
    if manifest.status == GateStatus::NeedsResourceRunner {
        return GateReport {
            status: GateStatus::NeedsResourceRunner,
            metrics: Vec::new(),
            leading_metrics: 0,
            reasons: manifest.notes.clone(),
        };
    }
    let mut metrics = Vec::new();
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
                    name: name.into(),
                    wjsm_numerator: None,
                    jdk_numerator: None,
                    denominator: None,
                });
            }
            continue;
        };
        for name in metric_names() {
            metrics.push(paired_metric(name, wjsm, jdk));
        }
    }
    if metrics.is_empty() {
        return GateReport {
            status: GateStatus::NeedsVerification,
            metrics: Vec::new(),
            leading_metrics: 0,
            reasons: vec!["manifest contains no WJSM/JDK comparison pairs".into()],
        };
    }
    evaluate_gate(&metrics)
}

fn paired_metric(name: &str, wjsm: &RunReport, jdk: &RunReport) -> NormalizedMetric {
    let wjsm_value = mean_metric_value(wjsm, name);
    let jdk_value = mean_metric_value(jdk, name);
    NormalizedMetric {
        name: format!("{}:{name}", wjsm.scenario.logical_graph_hash),
        // 输入已经是归一化值，使用共同 denominator=1 保留 ratio 语义。
        wjsm_numerator: wjsm_value,
        jdk_numerator: jdk_value,
        denominator: Some(1.0),
    }
}

fn mean_metric_value(report: &RunReport, name: &str) -> Option<f64> {
    let values: Vec<_> = report
        .samples
        .iter()
        .filter_map(|sample| {
            sample
                .metrics
                .iter()
                .find(|metric| metric.name == name)
                .and_then(|metric| metric.value)
        })
        .collect();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
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
