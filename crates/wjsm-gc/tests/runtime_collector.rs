use std::collections::HashSet;
use std::sync::Arc;

use wjsm_gc::{
    GcAlgorithmKind, HandleGeneration, HandleTableV2, HeapAccessV2, ManagedHeapLayout,
    PROTO_NULL_SENTINEL, RuntimeCollector, TestHeapMemory,
};
use wjsm_ir::constants;

const HEAP_BYTES: u64 = 1024 * 1024;
const PROPERTY_KEY: u32 = 17;

struct TestHeap {
    access: HeapAccessV2<TestHeapMemory>,
}

impl TestHeap {
    fn new() -> Self {
        let layout = Arc::new(ManagedHeapLayout::new(HEAP_BYTES, 64 * 1024).unwrap());
        let memory = TestHeapMemory::for_layout(&layout);
        let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
        Self {
            access: HeapAccessV2::with_handles(memory, layout, handles).unwrap(),
        }
    }

    fn object(&self, stored: u64) -> u32 {
        let handle = self.access.allocate_handle().unwrap();
        let bytes = u64::from(constants::HEAP_OBJECT_HEADER_SIZE)
            + u64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE);
        let (object, _) = self.access.reserve_nlab(bytes).unwrap();
        self.access
            .publish_object(handle, object, PROTO_NULL_SENTINEL, 1)
            .unwrap();
        self.access
            .set_property(handle, PROPERTY_KEY, stored)
            .unwrap();
        handle
    }

    fn property(&self, handle: u32) -> u64 {
        self.access
            .get_property(handle, PROPERTY_KEY)
            .unwrap()
            .unwrap()
    }
}

#[test]
fn mark_sweep_retires_only_dead_handles_without_moving_live_objects() {
    let heap = TestHeap::new();
    let live = heap.object(41);
    let dead = heap.object(99);
    let live_address = heap.access.resolve_handle(live).unwrap();
    let mut collector = RuntimeCollector::new(GcAlgorithmKind::MarkSweep);

    let report = collector
        .collect(heap.access.collector_capability(), &HashSet::from([live]))
        .unwrap();

    assert_eq!(report.retired_handles, [dead]);
    assert!(report.relocated_handles.is_empty());
    assert_eq!(heap.access.resolve_handle(live).unwrap(), live_address);
    assert!(heap.access.resolve_handle(dead).is_err());
    assert_eq!(heap.property(live), 41);
}

#[test]
fn g1_relocates_young_survivors_then_promotes_them() {
    let heap = TestHeap::new();
    let live = heap.object(42);
    let first_address = heap.access.resolve_handle(live).unwrap();
    let mut collector = RuntimeCollector::new(GcAlgorithmKind::G1);
    let roots = HashSet::from([live]);

    let first = collector
        .collect(heap.access.collector_capability(), &roots)
        .unwrap();
    let second_address = heap.access.resolve_handle(live).unwrap();
    assert_eq!(first.relocated_handles, [live]);
    assert!(first.promoted_handles.is_empty());
    assert_ne!(first_address, second_address);
    assert_eq!(
        heap.access.handle_generation(live),
        Some(HandleGeneration::Young)
    );

    let second = collector
        .collect(heap.access.collector_capability(), &roots)
        .unwrap();
    assert_eq!(second.relocated_handles, [live]);
    assert_eq!(second.promoted_handles, [live]);
    assert_ne!(heap.access.resolve_handle(live).unwrap(), second_address);
    assert_eq!(
        heap.access.handle_generation(live),
        Some(HandleGeneration::Old)
    );
    assert_eq!(heap.property(live), 42);
}

#[test]
fn zgc_relocates_all_live_objects_and_retires_dead_objects() {
    let heap = TestHeap::new();
    let first = heap.object(1);
    let second = heap.object(2);
    let dead = heap.object(3);
    let first_address = heap.access.resolve_handle(first).unwrap();
    let second_address = heap.access.resolve_handle(second).unwrap();
    let mut collector = RuntimeCollector::new(GcAlgorithmKind::Zgc);

    let report = collector
        .collect(
            heap.access.collector_capability(),
            &HashSet::from([first, second]),
        )
        .unwrap();

    assert_eq!(report.retired_handles, [dead]);
    assert_eq!(report.relocated_handles, [first, second]);
    assert_ne!(heap.access.resolve_handle(first).unwrap(), first_address);
    assert_ne!(heap.access.resolve_handle(second).unwrap(), second_address);
    assert_eq!(heap.property(first), 1);
    assert_eq!(heap.property(second), 2);
    assert_eq!(collector.telemetry_snapshot().cycles, 1);
}
