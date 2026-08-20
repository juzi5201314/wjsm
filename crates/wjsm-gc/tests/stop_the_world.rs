use std::cell::RefCell;
use std::sync::Arc;

use wjsm_gc::{
    GcAlgorithmKind, HandleGeneration, HandleTableV2, HeapAccessV2, ManagedHeapLayout, Nlab,
    PROTO_NULL_SENTINEL, PropertyKey, RootSnapshot, StopTheWorldCollector, TestHeapMemory,
};
use wjsm_ir::constants;

const HEAP_BYTES: u64 = 1024 * 1024;
const PROPERTY_KEY: PropertyKey = PropertyKey::from_name_id(17);

struct TestHeap {
    access: HeapAccessV2<TestHeapMemory>,
    nlab: RefCell<Nlab>,
}

impl TestHeap {
    fn new() -> Self {
        let layout = Arc::new(ManagedHeapLayout::new(HEAP_BYTES, 64 * 1024).unwrap());
        let memory = TestHeapMemory::for_layout(&layout);
        let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
        Self {
            access: HeapAccessV2::with_handles(
                memory,
                layout,
                handles,
                wjsm_gc::HeapBarrier::Disabled,
            )
            .unwrap(),
            nlab: RefCell::new(Nlab::new()),
        }
    }

    fn object(&self, stored: u64) -> u32 {
        let handle = self.access.allocate_handle().unwrap();
        let bytes = u64::from(constants::HEAP_OBJECT_HEADER_SIZE)
            + u64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE);
        let object = self
            .access
            .allocate(&mut self.nlab.borrow_mut(), bytes)
            .unwrap()
            .object()
            .offset();
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
    let mut collector = StopTheWorldCollector::new(GcAlgorithmKind::MarkSweep).unwrap();

    let roots = RootSnapshot::new(
        1,
        vec![wjsm_ir::value::encode_object_handle(live)],
        vec![],
        vec![],
    );
    let report = collector
        .collect(heap.access.collector_capability(), &roots)
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
    let mut collector = StopTheWorldCollector::new(GcAlgorithmKind::G1).unwrap();
    let roots = RootSnapshot::new(
        1,
        vec![wjsm_ir::value::encode_object_handle(live)],
        vec![],
        vec![],
    );

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
