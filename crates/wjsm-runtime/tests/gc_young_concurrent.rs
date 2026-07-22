#![cfg(feature = "managed-heap-v2")]

use std::time::Duration;

use wjsm_runtime::{
    GcRuntimeV2, HandleGeneration, HandleId, HandleTableV2, ManagedHeapLayout, YoungController,
    YoungPhase, publish_promotion,
};

const PAGE: u64 = 64 * 1024;

fn roots(handles: impl IntoIterator<Item = HandleId>) -> wjsm_runtime::RootSnapshot {
    let runtime = GcRuntimeV2::new();
    let mutator = runtime.register_mutator();
    runtime.request_root_snapshot();
    mutator.publish_roots(handles.into_iter().map(HandleId::get))
}

#[test]
fn young_mark_start_snapshots_roots_and_enables_black_allocation() {
    let young = YoungController::new(8);
    let child = HandleId::new(2);
    let root = HandleId::new(1);
    young.register_object(child, HandleGeneration::Young, [], false, false);
    young.register_object(root, HandleGeneration::Young, [Some(child)], false, false);
    let pause = young.pause_mark_start(&roots([root]));
    assert!(pause < Duration::from_millis(1));
    assert_eq!(young.phase(), YoungPhase::ConcurrentMark);
    assert!(young.epoch().young_marking);

    let newborn = HandleId::new(3);
    young.register_object(newborn, HandleGeneration::Young, [], false, false);
    assert!(young.is_marked(newborn));
    assert_eq!(young.report().black_allocations, 1);
}

#[test]
fn young_satb_keeps_overwritten_reference_and_terminates() {
    let young = YoungController::new(4);
    let lost = HandleId::new(10);
    let root = HandleId::new(11);
    let kept = HandleId::new(12);
    young.register_object(lost, HandleGeneration::Young, [], false, false);
    young.register_object(kept, HandleGeneration::Young, [], false, false);
    young.register_object(root, HandleGeneration::Young, [Some(lost)], false, false);
    young.pause_mark_start(&roots([root]));
    // mutator overwrites root.slot0 = kept, SATB must keep lost
    young.write_reference(root, 0, Some(kept), 0x100);
    while young.concurrent_mark_step(1) {}
    let pause = young.pause_mark_end();
    assert!(pause < Duration::from_millis(1));
    assert!(!young.pause_did_page_scan_or_copy());
    assert!(young.is_marked(root));
    assert!(young.is_marked(kept));
    assert!(young.is_marked(lost));
    assert!(young.terminated());
    assert!(young.report().satb_drained >= 1);
}

#[test]
fn remset_old_to_young_write_overwrite_delete_and_dedup() {
    let young = YoungController::new(8);
    let old = HandleId::new(20);
    let young_obj = HandleId::new(21);
    young.register_object(old, HandleGeneration::Old, [None], false, false);
    young.register_object(young_obj, HandleGeneration::Young, [], false, false);
    young.write_reference(old, 0, Some(young_obj), 0x2000);
    young.write_reference(old, 0, Some(young_obj), 0x2000); // dedup
    young.write_reference(old, 0, None, 0x2000); // delete
    assert!(young.remset().contains_slot(0x2000));
    assert_eq!(young.remset().active_len(), 1);

    let snap = young.remset().snapshot_and_flip();
    assert_eq!(snap, vec![0x2000]);
    assert_eq!(young.remset().active_len(), 0);
    // double buffer: new writes go to active while snapshot held
    young.write_reference(old, 0, Some(young_obj), 0x2008);
    assert!(young.remset().contains_slot(0x2008));
}

#[test]
fn promotion_dense_and_humongous_in_place() {
    let young = YoungController::new(4);
    let dense = HandleId::new(30);
    let humongous = HandleId::new(31);
    let sparse = HandleId::new(32);
    young.register_object(dense, HandleGeneration::Young, [], true, false);
    young.register_object(humongous, HandleGeneration::Young, [], false, true);
    young.register_object(sparse, HandleGeneration::Young, [], false, false);
    young.pause_mark_start(&roots([dense, humongous, sparse]));
    while young.concurrent_mark_step(8) {}
    young.pause_mark_end();
    let sparse_set = young.select_relocation_set();
    assert!(sparse_set.contains(&sparse));
    assert!(!sparse_set.contains(&dense));
    assert!(!sparse_set.contains(&humongous));
    assert_eq!(young.generation(dense), Some(HandleGeneration::Old));
    assert_eq!(young.generation(humongous), Some(HandleGeneration::Old));
    assert_eq!(young.report().promoted, 2);

    let layout = ManagedHeapLayout::new(PAGE * 4, PAGE).unwrap();
    let table = HandleTableV2::new(layout.clone()).unwrap();
    let handle = table.allocate_handle().unwrap();
    table
        .publish(handle, layout.object_heap_base(), HandleGeneration::Young)
        .unwrap();
    publish_promotion(&table, handle).unwrap();
    assert_eq!(
        table.resolve(handle).unwrap().generation(),
        HandleGeneration::Old
    );
}

#[test]
fn young_work_does_not_scale_with_old_heap_size() {
    let young = YoungController::new(16);
    // many old objects, few remset edges
    for i in 0..10_000u32 {
        young.register_object(HandleId::new(i), HandleGeneration::Old, [], false, false);
    }
    let root = HandleId::new(10_001);
    let child = HandleId::new(10_002);
    young.register_object(child, HandleGeneration::Young, [], false, false);
    young.register_object(root, HandleGeneration::Old, [Some(child)], false, false);
    young.write_reference(root, 0, Some(child), 0x9000);
    young.pause_mark_start(&roots([root]));
    while young.concurrent_mark_step(32) {}
    young.pause_mark_end();
    // bound is remset + marked young graph, not 10k old objects
    assert!(young.young_work_bound() < 100);
    assert!(young.is_marked(child));
}

#[test]
fn pause_mark_phases_under_one_millisecond() {
    let young = YoungController::new(4);
    let root = HandleId::new(1);
    young.register_object(root, HandleGeneration::Young, [], false, false);
    let start = young.pause_mark_start(&roots([root]));
    while young.concurrent_mark_step(4) {}
    let end = young.pause_mark_end();
    let relocate = young.pause_relocate_start();
    assert!(start < Duration::from_millis(1));
    assert!(end < Duration::from_millis(1));
    assert!(relocate < Duration::from_millis(1));
    assert!(young.report().pause_ns_max < 1_000_000);
}
