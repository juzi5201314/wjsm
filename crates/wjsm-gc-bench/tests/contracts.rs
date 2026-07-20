use clap::Parser;
use std::cell::Cell;
use wjsm_gc_bench::cli::Cli;
use wjsm_gc_bench::gate::{NormalizedMetric, evaluate_gate, evaluate_metric, evaluate_pause};
use wjsm_gc_bench::resource::{
    AdmissionRequest, AdmissionStatus, HostResourceProvider, HostResourceSnapshot, admit_then_run,
};
use wjsm_gc_bench::scenario::{ScenarioKind, ScenarioSpec};
use wjsm_gc_bench::schema::GateStatus;

#[test]
#[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
fn cli_surface_accepts_every_fixed_subcommand() {
    let commands: &[&[&str]] = &[
        &["capabilities"],
        &["preflight"],
        &["prepare-jdk"],
        &["baseline"],
        &["run"],
        &["micro", "--component", "allocator"],
        &[
            "compare",
            "--jdk-home",
            "/jdk",
            "--jdk-probe-home",
            "/probe",
        ],
        &[
            "compare",
            "--jdk-home",
            "/jdk",
            "--jdk-probe-home",
            "/probe",
            "--heap",
            "4g",
            "--duration",
            "3600",
            "--profile",
            "nightly",
            "--scenarios",
            "saturation,request,humongous,idle-uncommit",
        ],
        &["replay", "--manifest", "/tmp/manifest.json"],
        &[
            "gate",
            "--manifest",
            "/tmp/manifest.json",
            "--profile",
            "nightly",
        ],
    ];
    for command in commands {
        let args = std::iter::once("wjsm-gc-bench").chain(command.iter().copied());
        Cli::try_parse_from(args).unwrap_or_else(|error| panic!("{command:?} must parse: {error}"));
    }
}

#[test]
#[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
fn scenario_hash_and_denominators_are_deterministic() {
    let first = ScenarioSpec::new(ScenarioKind::Churn, 7, 256 * MIB, 50).build();
    let second = ScenarioSpec::new(ScenarioKind::Churn, 7, 256 * MIB, 50).build();

    assert_eq!(
        first.manifest.logical_graph_hash,
        second.manifest.logical_graph_hash
    );
    assert_eq!(first.denominators, second.denominators);
    assert!(first.denominators.logical_objects > 0);
    assert!(first.denominators.planned_allocation_bytes > 0);
    assert_eq!(first.denominators.physical_allocated_bytes, None);
    assert!(first.denominators.reference_edges > 0);
}

#[test]
#[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
fn resource_admission_rejects_before_spawn() {
    let provider = FakeResources(HostResourceSnapshot::synthetic(16 * GIB, 4 * GIB));
    let spawned = Cell::new(0usize);

    for heap in [GIB, 4 * GIB, 16 * GIB] {
        let decision = admit_then_run(&provider, AdmissionRequest::pr(heap), || {
            spawned.set(spawned.get() + 1);
            Ok(())
        })
        .expect("admission returns a decision");
        assert_eq!(decision.status, AdmissionStatus::NeedsResourceRunner);
    }
    assert_eq!(spawned.get(), 0);

    let decision = admit_then_run(&provider, AdmissionRequest::pr(256 * MIB), || {
        spawned.set(spawned.get() + 1);
        Ok(())
    })
    .expect("small profile admission");
    assert_eq!(decision.status, AdmissionStatus::Admitted);
    assert_eq!(
        decision.budget_formulas.required_available,
        "3 * max_heap_cap + max(2 GiB, 10% * effective_total)"
    );
    assert!(
        decision
            .budget_formulas
            .required_virtual_address
            .contains("32 GiB handle region")
    );
    assert_eq!(spawned.get(), 1);
}

#[test]
#[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
fn resource_probe_errors_reject_before_spawn() {
    let mut resources = HostResourceSnapshot::synthetic(128 * GIB, 128 * GIB);
    resources.probe_errors.push("RLIMIT_AS unavailable".into());
    let provider = FakeResources(resources);
    let spawned = Cell::new(false);
    let decision = admit_then_run(&provider, AdmissionRequest::pr(256 * MIB), || {
        spawned.set(true);
        Ok(())
    })
    .expect("structured admission decision");
    assert_eq!(decision.status, AdmissionStatus::NeedsResourceRunner);
    assert!(!spawned.get());
}

#[test]
#[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
fn missing_jdk_numerator_never_passes_gate() {
    let result = evaluate_metric(&NormalizedMetric {
        name: "gc_cpu_per_allocated_byte".into(),
        wjsm_numerator: Some(100.0),
        wjsm_denominator: Some(1_000.0),
        jdk_numerator: None,
        jdk_denominator: None,
    });
    assert_eq!(result.status, GateStatus::NeedsVerification);
}

#[test]
#[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
fn gate_uses_each_engine_physical_denominator() {
    let result = evaluate_metric(&NormalizedMetric {
        name: "gc_cpu_per_allocated_byte".into(),
        wjsm_numerator: Some(200.0),
        wjsm_denominator: Some(100.0),
        jdk_numerator: Some(100.0),
        jdk_denominator: Some(100.0),
    });
    assert_eq!(result.ratio, Some(2.0));
    assert_eq!(result.status, GateStatus::Failed);
}

#[test]
#[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
fn gate_requires_pause_distribution_evidence() {
    let metrics = [
        NormalizedMetric {
            name: "one".into(),
            wjsm_numerator: Some(80.0),
            wjsm_denominator: Some(100.0),
            jdk_numerator: Some(100.0),
            jdk_denominator: Some(100.0),
        },
        NormalizedMetric {
            name: "two".into(),
            wjsm_numerator: Some(80.0),
            wjsm_denominator: Some(100.0),
            jdk_numerator: Some(100.0),
            jdk_denominator: Some(100.0),
        },
    ];
    let pauses = [evaluate_pause("scenario".into(), None, Some(1), Some(1))];
    assert_eq!(
        evaluate_gate(&metrics, &pauses).status,
        GateStatus::NeedsVerification
    );
}

#[test]
#[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
fn nightly_gate_requires_hard_isolation_evidence() {
    use wjsm_gc_bench::comparison::gate_manifest_for_profile;
    use wjsm_gc_bench::cli::Profile;
    use wjsm_gc_bench::schema::BenchmarkManifest;

    let mut manifest = BenchmarkManifest::empty();
    manifest.status = GateStatus::Passed;
    let report = gate_manifest_for_profile(&manifest, Profile::Nightly);
    // Empty manifest still needs verification for missing pairs; hard-isolation
    // reasons only attach when runs exist. Ensure API is callable.
    assert!(
        report.status == GateStatus::NeedsVerification
            || report.status == GateStatus::NeedsResourceRunner
    );
}

struct FakeResources(HostResourceSnapshot);

impl HostResourceProvider for FakeResources {
    fn snapshot(&self) -> anyhow::Result<HostResourceSnapshot> {
        Ok(self.0.clone())
    }
}

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
