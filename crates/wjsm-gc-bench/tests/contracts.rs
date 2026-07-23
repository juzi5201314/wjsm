use clap::Parser;
use wjsm_gc_bench::cli::{Cli, GcKind};
use wjsm_gc_bench::scenario::{Scenario, ScenarioKind};

#[test]
fn cli_parses_run_defaults() {
    let cli = Cli::parse_from(["wjsm-gc-bench", "run"]);
    match cli.command {
        wjsm_gc_bench::cli::Command::Run(args) => {
            assert_eq!(args.gc, GcKind::Zgc);
            assert_eq!(args.samples, 10);
            assert_eq!(args.live_set, 50);
            assert_eq!(args.common.heap, 256 * 1024 * 1024);
        }
        _ => panic!("expected Run"),
    }
}

#[test]
fn cli_parses_info() {
    let cli = Cli::parse_from(["wjsm-gc-bench", "info", "--output", "/tmp/info.json"]);
    assert!(matches!(cli.command, wjsm_gc_bench::cli::Command::Info(_)));
}

#[test]
fn scenario_deterministic() {
    let a = Scenario::build(ScenarioKind::Churn, 42, 32 * 1024 * 1024, 50);
    let b = Scenario::build(ScenarioKind::Churn, 42, 32 * 1024 * 1024, 50);
    assert_eq!(a.source, b.source);
    assert_eq!(a.allocations, b.allocations);
    assert_eq!(a.retained, b.retained);
}

#[test]
fn all_scenarios_produce_source() {
    for kind in ScenarioKind::all() {
        let s = Scenario::build(*kind, 1, 32 * 1024 * 1024, 50);
        assert!(!s.source.is_empty(), "scenario {} has empty source", kind.as_str());
        assert!(s.source.contains("console.log"), "scenario {} missing output", kind.as_str());
    }
}
