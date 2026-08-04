use clap::Parser;
use wjsm_bench::cli::{Cli, EffectiveConfig};
use wjsm_bench::runner::{parse_max_rss, parse_ns_per_op};

#[test]
fn cli_parses_defaults() {
    let cli = Cli::parse_from(["wjsm-bench"]);
    assert_eq!(cli.scenarios, "");
    assert_eq!(cli.runtimes, "node,wjsm");
    assert!(!cli.cold);
    assert_eq!(cli.runs, 10);
    assert_eq!(cli.warmup, 3);
    assert!(!cli.quick);
    assert_eq!(cli.window_ms, 1000);
    assert_eq!(cli.warmup_ms, 500);
    assert_eq!(cli.output, None);
}

#[test]
fn quick_expands_smoke_config() {
    let cli = Cli::parse_from(["wjsm-bench", "--quick"]);
    assert_eq!(
        cli.effective(),
        EffectiveConfig {
            runs: 3,
            warmup: 1,
            window_ms: 200,
            warmup_ms: 500
        }
    );
}

#[test]
fn explicit_flags_win_without_quick() {
    let cli = Cli::parse_from([
        "wjsm-bench",
        "--runs",
        "5",
        "--warmup",
        "2",
        "--window-ms",
        "400",
    ]);
    assert_eq!(
        cli.effective(),
        EffectiveConfig {
            runs: 5,
            warmup: 2,
            window_ms: 400,
            warmup_ms: 500
        }
    );
}

#[test]
fn quick_overrides_explicit_flags() {
    let cli = Cli::parse_from([
        "wjsm-bench",
        "--quick",
        "--runs",
        "50",
        "--window-ms",
        "9000",
    ]);
    assert_eq!(
        cli.effective(),
        EffectiveConfig {
            runs: 3,
            warmup: 1,
            window_ms: 200,
            warmup_ms: 500
        }
    );
}

#[test]
fn parse_ns_per_op_regular() {
    let stdout = "ns_per_op=7712.5 iterations=129589\n";
    assert_eq!(parse_ns_per_op(stdout), Some(7712.5));
}

#[test]
fn parse_ns_per_op_infinity() {
    let stdout = "ns_per_op=Infinity iterations=0\n";
    assert_eq!(parse_ns_per_op(stdout), None);
}

#[test]
fn parse_ns_per_op_missing() {
    assert_eq!(parse_ns_per_op("no such line\n"), None);
}

#[test]
fn parse_hyperfine_wall_json() {
    // 最小 hyperfine --export-json 结构，与 1.20 输出形状一致。
    let payload = r#"{
      "results": [
        {
          "command": "node /x.js > /dev/null",
          "mean": 0.05,
          "stddev": 0.001,
          "median": 0.049,
          "user": 0.0,
          "system": 0.0,
          "min": 0.048,
          "max": 0.052,
          "times": [0.048, 0.049, 0.052]
        },
        {
          "command": "wjsm run /x.js > /dev/null",
          "mean": 0.3,
          "stddev": 0.01,
          "median": 0.29,
          "user": 0.0,
          "system": 0.0,
          "min": 0.28,
          "max": 0.31,
          "times": [0.28, 0.29, 0.31]
        }
      ]
    }"#;
    let path = std::env::temp_dir().join("wjsm-bench-contract-hyperfine.json");
    std::fs::write(&path, payload).unwrap();
    let commands = vec![
        (
            wjsm_bench::runner::RuntimeKind::Node,
            "node /x.js".to_owned(),
        ),
        (
            wjsm_bench::runner::RuntimeKind::Wjsm,
            "wjsm run /x.js".to_owned(),
        ),
    ];
    let parsed = wjsm_bench::runner::parse_wall_json(&path, &commands).unwrap();
    assert_eq!(parsed.len(), 2);
    let node = &parsed["node"];
    assert_eq!(node.median_s, 0.049);
    assert_eq!(node.runs, 3);
    let wjsm = &parsed["wjsm"];
    assert_eq!(wjsm.mean_s, 0.3);
    assert_eq!(wjsm.runs, 3);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn parse_max_rss_extracts_kbytes() {
    let stderr = "Command being timed: \"node /x.js\"\n\
                  \tUser time (seconds): 0.02\n\
                  Maximum resident set size (kbytes): 48640\n";
    assert_eq!(parse_max_rss(stderr), Some(48640));
}

#[test]
fn parse_max_rss_missing() {
    assert_eq!(parse_max_rss("no rss here\n"), None);
}
