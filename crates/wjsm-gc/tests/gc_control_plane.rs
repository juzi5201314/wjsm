use wjsm_gc::{CollectorContext, GcEdge, GcEphemeron, GcRuntimeV2, MutatorContext, RootSnapshot};
use wjsm_ir::value;

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

    let values = [
        value::encode_object_handle(7),
        value::encode_handle(value::TAG_FUNCTION, 11),
        value::encode_runtime_string_handle(13),
    ];
    let snapshot = mutator.publish_roots(values);
    assert_eq!(snapshot.roots(), &values);
    assert_eq!(snapshot.root_handles().collect::<Vec<_>>(), [7, 11, 13]);
    assert!(collector.observe_roots(&snapshot));
    assert_eq!(collector.observed_epoch(), snapshot.epoch());
    assert_eq!(runtime.active_mutators(), 1);
    assert_eq!(runtime.active_collectors(), 1);
}

#[test]
fn root_snapshot_sorts_host_edges_and_ephemerons() {
    let snapshot = RootSnapshot::new(
        9,
        vec![value::encode_object_handle(1)],
        vec![
            GcEdge {
                owner: 3,
                target: 4,
            },
            GcEdge {
                owner: 1,
                target: 2,
            },
        ],
        vec![
            GcEphemeron {
                owner: 7,
                key: 8,
                value: 9,
            },
            GcEphemeron {
                owner: 4,
                key: 5,
                value: 6,
            },
        ],
    );
    assert_eq!(
        snapshot.strong_edges()[0],
        GcEdge {
            owner: 1,
            target: 2
        }
    );
    assert_eq!(
        snapshot.ephemerons()[0],
        GcEphemeron {
            owner: 4,
            key: 5,
            value: 6,
        }
    );
}

#[test]
fn controller_requests_monotonic_epochs_without_algorithm_mutex() {
    let runtime = GcRuntimeV2::new();
    let first = runtime.request_root_snapshot();
    let second = runtime.request_root_snapshot();

    assert!(second > first);
    assert_eq!(runtime.requested_epoch(), second);
}
