use wjsm_runtime::{GcStats, GcTelemetry};

#[test]
fn gc_telemetry_snapshot_is_versioned_and_uses_hdr_histograms() {
    let telemetry = GcTelemetry::default();
    let mut stats = GcStats::default();
    stats.pause_ns_max = 42_000;
    stats.pause_ns_total = 42_000;
    stats.pause_count = 1;
    stats.freed_bytes = 4096;
    stats.relocated_bytes = 1024;
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
