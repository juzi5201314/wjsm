use std::collections::BTreeSet;

use wjsm_ir::value::encode_object_handle;
use wjsm_runtime::realm::{Realm, RealmId, RealmIntrinsics};
use wjsm_runtime::{
    ConcurrentHostRoots, HandleGeneration, HandleId, HandleTableV2, ManagedHeapLayout,
    V2ConditionalRoots, WeakState,
};

const PAGE: u64 = 64 * 1024;

fn table() -> HandleTableV2 {
    let layout = ManagedHeapLayout::new(PAGE * 4, PAGE).unwrap();
    HandleTableV2::new(layout).unwrap()
}

fn publish(table: &HandleTableV2) -> HandleId {
    let handle = table.allocate_handle().unwrap();
    table
        .publish(
            handle,
            table.layout().object_heap_base() + u64::from(handle.get()) * 16,
            HandleGeneration::Young,
        )
        .unwrap();
    handle
}

#[test]
fn side_table_does_not_reverse_keep_and_finalizer_runs_once() {
    let table = table();
    let live_obj = publish(&table);
    let dead_obj = publish(&table);
    let hosts = ConcurrentHostRoots::new();
    hosts.register_weak(1, dead_obj);
    hosts.register_finalizer(7, dead_obj, 99);
    hosts.push_side_table_value(encode_object_handle(dead_obj.get()));

    let live = BTreeSet::from([live_obj]);
    let conditional = V2ConditionalRoots::default();
    let _ = hosts.cleanup_before_quarantine(&live, &table, &conditional);
    assert_eq!(hosts.weak_state(1), Some(WeakState::Cleared));
    assert!(hosts.report().cleanup_before_quarantine);

    table.retire(dead_obj).unwrap();
    hosts.publish_cycle_and_run_callbacks();
    assert!(hosts.finalizer_ran_once(7));
    hosts.publish_cycle_and_run_callbacks();
    assert_eq!(hosts.report().finalizers_ran, 1);
    assert!(hosts.report().callbacks_after_publish);
    assert!(hosts.report().side_table_filtered >= 1);
}

#[test]
fn realm_destroy_and_snapshot_restore_weak_cleanup() {
    let table = table();
    let global = publish(&table);
    let dead = publish(&table);
    let mut intrinsics = RealmIntrinsics::empty();
    intrinsics.object_proto = encode_object_handle(global.get());
    let realm = Realm::new(RealmId(1), encode_object_handle(global.get()), intrinsics);
    let mut conditional = V2ConditionalRoots::default();
    conditional.push_realm(realm);

    let hosts = ConcurrentHostRoots::new();
    hosts.register_weak(2, dead);
    let live = BTreeSet::from([global]);
    let filtered = hosts.realm_destroy_filter(&conditional, &table, &live);
    assert!(filtered.contains(&global));
    assert!(!filtered.contains(&dead));

    hosts.snapshot_restore_weak_cleanup(&live, &table);
    assert_eq!(hosts.weak_state(2), Some(WeakState::Cleared));
}
