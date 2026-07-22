use wjsm_runtime::{CollectorContext, GcRuntimeV2, MutatorContext, RootSnapshot};

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn collector_context_is_send_sync_and_store_free() {
    assert_send_sync::<CollectorContext>();
    assert_send_sync::<GcRuntimeV2>();
    assert_send_sync::<MutatorContext>();
    assert_send_sync::<RootSnapshot>();
}

#[test]
fn mutator_publishes_immutable_root_snapshot_to_collector() {
    let runtime = GcRuntimeV2::new();
    let mutator = runtime.register_mutator();
    let collector = runtime.register_collector();

    let snapshot = mutator.publish_roots([7, 11, 13]);
    assert_eq!(snapshot.handles(), &[7, 11, 13]);
    assert!(collector.observe_roots(&snapshot));
    assert_eq!(collector.observed_epoch(), snapshot.epoch());
    assert_eq!(runtime.active_mutators(), 1);
    assert_eq!(runtime.active_collectors(), 1);
}

#[test]
fn controller_requests_monotonic_epochs_without_algorithm_mutex() {
    let runtime = GcRuntimeV2::new();
    let first = runtime.request_root_snapshot();
    let second = runtime.request_root_snapshot();

    assert!(second > first);
    assert_eq!(runtime.requested_epoch(), second);
}
