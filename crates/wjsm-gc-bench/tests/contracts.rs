use clap::Parser;
use std::cell::Cell;
use wjsm_gc_bench::cli::Cli;
use wjsm_gc_bench::gate::{NormalizedMetric, evaluate_metric};
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
        &["replay", "--manifest", "/tmp/manifest.json"],
        &["gate", "--manifest", "/tmp/manifest.json"],
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
fn missing_jdk_numerator_never_passes_gate() {
    let result = evaluate_metric(&NormalizedMetric {
        name: "gc_cpu_per_allocated_byte".into(),
        wjsm_numerator: Some(100.0),
        jdk_numerator: None,
        denominator: Some(1_000.0),
    });
    assert_eq!(result.status, GateStatus::NeedsVerification);
}

struct FakeResources(HostResourceSnapshot);

impl HostResourceProvider for FakeResources {
    fn snapshot(&self) -> anyhow::Result<HostResourceSnapshot> {
        Ok(self.0.clone())
    }
}

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
