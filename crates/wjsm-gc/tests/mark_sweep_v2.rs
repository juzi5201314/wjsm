use wjsm_gc::{
    GcRuntimeV2, HandleId, ManagedHeapLayout, MarkSweepV2, NativeHeapMemory, RootSnapshot,
};

const PAGE_BYTES: u64 = 64 * 1024;

fn native_heap(layout: &ManagedHeapLayout) -> NativeHeapMemory {
    NativeHeapMemory::for_layout(layout)
}

fn root_snapshot(handles: impl IntoIterator<Item = HandleId>) -> RootSnapshot {
    let runtime = GcRuntimeV2::new();
    let mutator = runtime.register_mutator();
    runtime.request_root_snapshot();
    mutator.publish_roots(handles.into_iter().map(HandleId::get))
}

#[test]
fn mark_sweep_v2_marks_roots_retires_dead_handles_and_reclaims_empty_pages() {
    let layout = ManagedHeapLayout::new(PAGE_BYTES * 16, PAGE_BYTES).unwrap();
    let collector = MarkSweepV2::new(native_heap(&layout), layout).unwrap();
    let live_child = collector.allocate(PAGE_BYTES, []).unwrap();
    let live_root = collector.allocate(PAGE_BYTES, [live_child]).unwrap();
    let dead = collector.allocate(PAGE_BYTES, []).unwrap();
    let snapshot = root_snapshot([live_root]);
    let participant = collector.register_epoch_participant();
    participant.enter();
    let mut cleaned = Vec::new();

    let report = collector
        .collect(&snapshot, |handle| cleaned.push(handle))
        .unwrap();

    assert_eq!(report.marked, 2);
    assert_eq!(report.retired, 1);
    assert_eq!(report.reclaimed_handles, 0);
    assert_eq!(report.reclaimed_dedicated_pages, 1);
    assert_eq!(cleaned, vec![dead]);
    assert!(collector.is_marked(live_root));
    assert!(collector.is_marked(live_child));
    assert!(collector.resolve(live_root).is_some());
    assert!(collector.resolve(live_child).is_some());
    assert!(collector.resolve(dead).is_none());

    participant.exit();
    let reclaim = collector.collect(&snapshot, |_| {}).unwrap();
    assert_eq!(reclaim.reclaimed_handles, 1);
    assert_eq!(collector.allocate(PAGE_BYTES, []).unwrap(), dead);
}

#[test]
fn mark_sweep_v2_allocation_oom_runs_full_collection_before_retry() {
    let layout = ManagedHeapLayout::new(PAGE_BYTES * 2, PAGE_BYTES).unwrap();
    let collector = MarkSweepV2::new(native_heap(&layout), layout).unwrap();
    let live = collector.allocate(PAGE_BYTES, []).unwrap();
    let dead = collector.allocate(PAGE_BYTES, []).unwrap();
    let snapshot = root_snapshot([live]);
    let mut cleaned = Vec::new();

    let allocation = collector
        .allocate_or_collect(PAGE_BYTES, [], &snapshot, |handle| cleaned.push(handle))
        .unwrap();

    let report = allocation.full_collection.unwrap();
    assert_eq!(report.marked, 1);
    assert_eq!(report.retired, 1);
    assert_eq!(report.reclaimed_dedicated_pages, 1);
    assert_eq!(cleaned, vec![dead]);
    assert_eq!(allocation.handle, dead);
    assert!(collector.resolve(live).is_some());
    assert!(collector.resolve(allocation.handle).is_some());
}
