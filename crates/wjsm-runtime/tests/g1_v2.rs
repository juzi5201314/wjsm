#![cfg(feature = "managed-heap-v2")]

use wasmtime::{MemoryType, SharedMemory};
use wjsm_engine_config::EngineConfig;
use wjsm_runtime::{
    G1V2, G1V2Generation, GcRuntimeV2, HandleId, ManagedHeapLayout, RootSnapshot, SharedHeapMemory,
};

const HANDLE_REGION_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const PAGE_BYTES: u64 = 64 * 1024;

fn shared_heap() -> SharedHeapMemory {
    let engine = EngineConfig::artifact().build().unwrap();
    let memory = SharedMemory::new(
        &engine,
        MemoryType::builder()
            .memory64(true)
            .shared(true)
            .min(HANDLE_REGION_BYTES / PAGE_BYTES + 1)
            .max(Some(HANDLE_REGION_BYTES / PAGE_BYTES + 64))
            .build()
            .unwrap(),
    )
    .unwrap();
    SharedHeapMemory::new(memory)
}

fn root_snapshot(handles: impl IntoIterator<Item = HandleId>) -> RootSnapshot {
    let runtime = GcRuntimeV2::new();
    let mutator = runtime.register_mutator();
    runtime.request_root_snapshot();
    mutator.publish_roots(handles.into_iter().map(HandleId::get))
}

#[test]
fn g1_v2_young_evacuation_preserves_identity_and_promotes_survivors() {
    let layout = ManagedHeapLayout::new(PAGE_BYTES * 12, PAGE_BYTES).unwrap();
    let collector = G1V2::new(shared_heap(), layout, 2).unwrap();
    let child = collector.allocate_young(PAGE_BYTES, []).unwrap();
    let root = collector.allocate_young(PAGE_BYTES, [child]).unwrap();
    let dead = collector.allocate_young(PAGE_BYTES, []).unwrap();
    collector.write_payload(child, &[42]).unwrap();
    let child_address = collector.address(child).unwrap();
    let root_address = collector.address(root).unwrap();
    let snapshot = root_snapshot([root]);
    let mut cleaned = Vec::new();

    let young = collector
        .collect_young(&snapshot, |handle| cleaned.push(handle))
        .unwrap();

    assert_eq!(young.evacuated, 2);
    assert_eq!(young.retired, 1);
    assert_eq!(cleaned, vec![dead]);
    assert_ne!(collector.address(child).unwrap(), child_address);
    assert_ne!(collector.address(root).unwrap(), root_address);
    assert_eq!(collector.read_payload(child, 1).unwrap(), vec![42]);
    assert_eq!(collector.generation(child), Some(G1V2Generation::Survivor));
    assert_eq!(collector.generation(root), Some(G1V2Generation::Survivor));

    let promoted = collector.collect_young(&snapshot, |_| {}).unwrap();
    assert_eq!(promoted.promoted, 2);
    assert_eq!(collector.generation(child), Some(G1V2Generation::Old));
    assert_eq!(collector.generation(root), Some(G1V2Generation::Old));
}

#[test]
fn g1_v2_remembered_old_to_young_edge_survives_young_collection() {
    let layout = ManagedHeapLayout::new(PAGE_BYTES * 8, PAGE_BYTES).unwrap();
    let collector = G1V2::new(shared_heap(), layout, 2).unwrap();
    let old = collector.allocate_old(1024, []).unwrap();
    let young = collector.allocate_young(1024, []).unwrap();
    collector
        .record_reference_write(old, 0, Some(young))
        .unwrap();

    let report = collector.collect_young(&root_snapshot([]), |_| {}).unwrap();

    assert_eq!(report.remembered_cards_scanned, 1);
    assert!(collector.address(young).is_some());
}

#[test]
fn g1_v2_retains_old_to_survivor_edges_across_young_collections() {
    let layout = ManagedHeapLayout::new(PAGE_BYTES * 8, PAGE_BYTES).unwrap();
    let collector = G1V2::new(shared_heap(), layout, 2).unwrap();
    let old = collector.allocate_old(1024, []).unwrap();
    let young = collector.allocate_young(1024, []).unwrap();
    collector.write_payload(young, &[42]).unwrap();
    collector
        .record_reference_write(old, 0, Some(young))
        .unwrap();

    collector.collect_young(&root_snapshot([]), |_| {}).unwrap();
    let next = collector.collect_young(&root_snapshot([]), |_| {}).unwrap();

    assert_eq!(next.retired, 0);
    assert_eq!(collector.read_payload(young, 1).unwrap(), vec![42]);
    assert_eq!(collector.generation(young), Some(G1V2Generation::Old));
}

#[test]
fn g1_v2_redirties_promoted_objects_with_surviving_young_children() {
    let layout = ManagedHeapLayout::new(PAGE_BYTES * 12, PAGE_BYTES).unwrap();
    let collector = G1V2::new(shared_heap(), layout, 2).unwrap();
    let parent = collector.allocate_young(1024, []).unwrap();

    collector
        .collect_young(&root_snapshot([parent]), |_| {})
        .unwrap();
    let child = collector.allocate_young(1024, []).unwrap();
    collector.write_payload(child, &[42]).unwrap();
    collector
        .record_reference_write(parent, 0, Some(child))
        .unwrap();
    collector
        .collect_young(&root_snapshot([parent]), |_| {})
        .unwrap();

    let next = collector.collect_young(&root_snapshot([]), |_| {}).unwrap();

    assert!(next.remembered_cards_scanned > 0);
    assert_eq!(next.retired, 0);
    assert_eq!(collector.read_payload(child, 1).unwrap(), vec![42]);
    assert_eq!(collector.generation(child), Some(G1V2Generation::Old));
}

#[test]
fn g1_v2_promotion_failure_keeps_handle_and_promotes_in_place() {
    let layout = ManagedHeapLayout::new(PAGE_BYTES, PAGE_BYTES).unwrap();
    let collector = G1V2::new(shared_heap(), layout, 2).unwrap();
    let root = collector.allocate_young(PAGE_BYTES, []).unwrap();
    let address = collector.address(root).unwrap();

    let report = collector
        .collect_young(&root_snapshot([root]), |_| {})
        .unwrap();

    assert!(report.promotion_failed);
    assert_eq!(report.promoted, 1);
    assert_eq!(collector.address(root), Some(address));
    assert_eq!(collector.generation(root), Some(G1V2Generation::Old));
}

#[test]
fn g1_v2_records_collection_telemetry() {
    let layout = ManagedHeapLayout::new(PAGE_BYTES * 4, PAGE_BYTES).unwrap();
    let collector = G1V2::new(shared_heap(), layout, 2).unwrap();
    let root = collector.allocate_young(1024, []).unwrap();

    collector
        .collect_young(&root_snapshot([root]), |_| {})
        .unwrap();
    collector.collect_full(&root_snapshot([]), |_| {}).unwrap();
    let telemetry = collector.telemetry_snapshot();

    assert_eq!(telemetry.collector, "g1-v2");
    assert_eq!(telemetry.cycles, 2);
    assert_eq!(telemetry.pause.count, 2);
    assert!(telemetry.relocated_bytes >= 1024);
    assert!(telemetry.reclaimed_bytes >= 1024);
}

#[test]
fn g1_v2_mixed_and_full_collections_reclaim_old_and_humongous_pages() {
    let layout = ManagedHeapLayout::new(PAGE_BYTES * 10, PAGE_BYTES).unwrap();
    let collector = G1V2::new(shared_heap(), layout, 2).unwrap();
    let live_old = collector.allocate_old(1024, []).unwrap();
    let dead_old = collector.allocate_old(1024, []).unwrap();
    let old_address = collector.address(live_old).unwrap();
    let mixed = collector
        .collect_mixed(&root_snapshot([live_old]), |_| {})
        .unwrap();

    assert_eq!(mixed.retired, 1);
    assert_eq!(mixed.evacuated, 1);
    assert_ne!(collector.address(live_old), Some(old_address));
    assert!(collector.address(dead_old).is_none());

    let dead_humongous = collector.allocate_old(PAGE_BYTES * 2, []).unwrap();
    let live_identity = live_old;
    let full = collector
        .collect_full(&root_snapshot([live_old]), |_| {})
        .unwrap();

    assert_eq!(full.retired, 1);
    assert!(full.reclaimed_pages >= 2);
    assert_eq!(live_old, live_identity);
    assert!(collector.address(live_old).is_some());
    assert!(collector.address(dead_humongous).is_none());
}
