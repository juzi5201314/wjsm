use wjsm_runtime::{GcExecutionStats, GcStats, GcTelemetry};

#[test]
fn gc_telemetry_snapshot_is_versioned_and_uses_hdr_histograms() {
    let telemetry = GcTelemetry::default();
    let stats = GcStats {
        pause_ns_max: 42_000,
        pause_ns_total: 42_000,
        pause_count: 1,
        freed_bytes: 4096,
        relocated_bytes: 1024,
        ..Default::default()
    };
    telemetry.record_cycle("zgc", &stats);

    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.schema_version, 1);
    assert_eq!(snapshot.cycles, 1);
    assert_eq!(snapshot.reclaimed_bytes, 4096);
    assert_eq!(snapshot.relocated_bytes, 1024);
    assert_eq!(snapshot.pause.count, 1);
    assert_eq!(snapshot.pause.max_ns, 42_000);
    assert_eq!(snapshot.pause.p99_ns, 42_015);

    let json = telemetry.to_json().expect("telemetry JSON");
    assert!(json.contains("\"schema_version\":1"));
    assert!(json.contains("\"collector\":\"zgc\""));
}

#[test]
fn execution_telemetry_uses_cumulative_gc_counters() {
    let last = GcStats {
        freed_bytes: 3,
        relocated_bytes: 5,
        ..Default::default()
    };
    let cumulative = GcStats {
        freed_bytes: 13,
        relocated_bytes: 17,
        ..Default::default()
    };
    let telemetry = GcTelemetry::from_execution_stats(
        "zgc",
        &GcExecutionStats {
            last,
            cumulative,
            pause_hist: vec![11, 13],
            ..Default::default()
        },
    );

    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.cycles, 2);
    assert_eq!(snapshot.reclaimed_bytes, 13);
    assert_eq!(snapshot.relocated_bytes, 17);
}
