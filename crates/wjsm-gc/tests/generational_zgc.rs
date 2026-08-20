use std::sync::Arc;
use std::time::Duration;
use wjsm_gc::{
    BarrierRecord, GcEdge, GcEphemeron, GcSafepointAction, GenerationalZgc, HandleGeneration,
    HandleTableV2, HeapAccessV2, HeapBarrier, ManagedHeapLayout, Nlab, PROTO_NULL_SENTINEL,
    PropertyKey, RootSnapshot, RuntimeGcReport, TestHeapMemory, ZgcBarrierSet,
};
use wjsm_ir::{constants, value};
fn zgc_heap() -> Arc<HeapAccessV2<TestHeapMemory>> {
    let layout = Arc::new(ManagedHeapLayout::new(1024 * 1024, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let barrier = Arc::new(ZgcBarrierSet::new(
        Arc::clone(&handles),
        memory.clone(),
        4_096,
    ));
    Arc::new(
        HeapAccessV2::with_handles(memory, layout, handles, HeapBarrier::Zgc(barrier)).unwrap(),
    )
}

fn publish_object(heap: &HeapAccessV2<TestHeapMemory>, nlab: &mut Nlab, capacity: u32) -> u32 {
    let handle = heap.allocate_handle().unwrap();
    let bytes = u64::from(constants::HEAP_OBJECT_HEADER_SIZE + capacity * 8);
    let object = heap.allocate(nlab, bytes).unwrap().object().offset();
    heap.publish_object(handle, object, PROTO_NULL_SENTINEL, capacity)
        .unwrap();
    handle
}

fn drive_automatic_cycle(
    collector: &GenerationalZgc<TestHeapMemory>,
    roots: &[i64],
) -> RuntimeGcReport {
    loop {
        match collector.safepoint_action() {
            GcSafepointAction::PublishRoots { epoch } => {
                if let Some(report) = collector
                    .at_safepoint(Some(RootSnapshot::new(
                        epoch,
                        roots.to_vec(),
                        Vec::new(),
                        Vec::new(),
                    )))
                    .unwrap()
                {
                    return report;
                }
            }
            GcSafepointAction::FlushBarriers | GcSafepointAction::FinishCycle => {
                if let Some(report) = collector.at_safepoint(None).unwrap() {
                    return report;
                }
            }
            GcSafepointAction::Idle => std::thread::yield_now(),
            GcSafepointAction::Assist { .. } => {
                collector.at_safepoint(None).unwrap();
            }
        }
    }
}
#[test]
fn full_cycle_marks_real_heap_slots_and_encoded_host_graph() {
    let heap = zgc_heap();
    let mut nlab = Nlab::new();
    let child = publish_object(&heap, &mut nlab, 0);
    let root = publish_object(&heap, &mut nlab, 1);
    heap.set_property(
        root,
        PropertyKey::from_name_id(1),
        value::encode_object_handle(child) as u64,
    )
    .unwrap();

    let host_root = value::encode_function_idx(7);

    let host_key = value::encode_runtime_string_handle(8);
    let host_value = value::encode_function_idx(9);
    let roots = RootSnapshot::new(
        1,
        vec![value::encode_object_handle(root), host_root],
        vec![GcEdge {
            owner: host_root,
            target: host_key,
        }],
        vec![GcEphemeron {
            owner: host_root,
            key: host_key,
            value: host_value,
        }],
    );
    let collector = GenerationalZgc::new(Arc::clone(&heap), 2, 64).unwrap();
    let report = collector.collect_full(roots).unwrap();

    assert_eq!(report.stats.marked, 2);
    assert_eq!(report.stats.mark_live_bytes, 56);
    let mut expected = vec![host_root, host_key, host_value];
    expected.sort_unstable();
    assert_eq!(report.live_host_values, expected);
    assert_eq!(collector.telemetry_snapshot().collector, "zgc");
}

#[test]
fn collector_rejects_disabled_barrier_and_resets_only_while_idle() {
    let zgc = zgc_heap();
    let collector = GenerationalZgc::new(Arc::clone(&zgc), 1, 16).unwrap();
    collector.reset_heap(zgc_heap()).unwrap();

    let layout = Arc::new(ManagedHeapLayout::new(1024 * 1024, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let disabled = Arc::new(
        HeapAccessV2::with_handles(memory, layout, handles, HeapBarrier::Disabled).unwrap(),
    );
    assert!(GenerationalZgc::new(disabled, 1, 16).is_err());
}

#[test]
fn sparse_page_relocates_survivor_then_promotes_at_age_two() {
    let heap = zgc_heap();
    let mut nlab = Nlab::new();
    let survivor = publish_object(&heap, &mut nlab, 1);
    heap.set_property(
        survivor,
        PropertyKey::from_name_id(1),
        value::encode_f64(42.0) as u64,
    )
    .unwrap();
    let dead = [
        publish_object(&heap, &mut nlab, 1),
        publish_object(&heap, &mut nlab, 1),
        publish_object(&heap, &mut nlab, 1),
    ];
    let source = heap.resolve_handle(survivor).unwrap();
    let collector = GenerationalZgc::new(Arc::clone(&heap), 2, 64).unwrap();
    let roots = || {
        RootSnapshot::new(
            1,
            vec![value::encode_object_handle(survivor)],
            Vec::new(),
            Vec::new(),
        )
    };

    let first = collector.collect_full(roots()).unwrap();
    assert_eq!(first.retired_handles, dead);
    assert_eq!(first.relocated_handles, [survivor]);
    assert_ne!(heap.resolve_handle(survivor).unwrap(), source);
    assert_eq!(
        heap.get_property(survivor, PropertyKey::from_name_id(1))
            .unwrap(),
        Some(value::encode_f64(42.0) as u64),
    );

    let second = collector.collect_full(roots()).unwrap();
    assert_eq!(second.promoted_handles, [survivor]);
    assert_eq!(
        heap.handle_generation(survivor),
        Some(wjsm_gc::HandleGeneration::Old)
    );
}

#[test]
fn remembered_slot_keeps_young_target_across_cycles() {
    let heap = zgc_heap();
    let mut nlab = Nlab::new();
    let owner = publish_object(&heap, &mut nlab, 4_000);
    heap.promote_to_old(owner).unwrap();
    let target = publish_object(&heap, &mut nlab, 1);
    let dead = [
        publish_object(&heap, &mut nlab, 1),
        publish_object(&heap, &mut nlab, 1),
        publish_object(&heap, &mut nlab, 1),
    ];
    heap.set_property(
        owner,
        PropertyKey::from_name_id(1),
        value::encode_object_handle(target) as u64,
    )
    .unwrap();
    let collector = GenerationalZgc::new(Arc::clone(&heap), 2, 64).unwrap();
    collector.observe_allocation(1024 * 1024, Duration::from_nanos(1));

    let first = drive_automatic_cycle(&collector, &[value::encode_object_handle(owner)]);
    assert_eq!(first.stats.cycle_kind, wjsm_gc::CycleKind::Young);
    assert_eq!(first.retired_handles, dead);
    assert_eq!(first.relocated_handles, [target]);

    collector.observe_allocation(1024 * 1024, Duration::from_nanos(1));
    let second = drive_automatic_cycle(&collector, &[value::encode_object_handle(owner)]);
    assert_eq!(second.stats.cycle_kind, wjsm_gc::CycleKind::Young);
    assert_eq!(second.promoted_handles, [target]);
    assert_eq!(heap.handle_generation(target), Some(HandleGeneration::Old));
}

#[test]
fn old_cycle_reaches_old_target_through_young_root() {
    let heap = zgc_heap();
    let mut nlab = Nlab::new();
    let target = publish_object(&heap, &mut nlab, 8_000);
    heap.promote_to_old(target).unwrap();
    let bridge = publish_object(&heap, &mut nlab, 1);
    heap.set_property(
        bridge,
        PropertyKey::from_name_id(1),
        value::encode_object_handle(target) as u64,
    )
    .unwrap();
    let dead: Vec<_> = (0..8)
        .map(|_| {
            let handle = publish_object(&heap, &mut nlab, 8_000);
            heap.promote_to_old(handle).unwrap();
            handle
        })
        .collect();
    let collector = GenerationalZgc::new(Arc::clone(&heap), 2, 128).unwrap();

    let report = drive_automatic_cycle(&collector, &[value::encode_object_handle(bridge)]);
    assert_eq!(report.stats.cycle_kind, wjsm_gc::CycleKind::ZgcCycle);
    assert_eq!(report.stats.marked, 1);
    assert_eq!(report.retired_handles, dead);
    assert_eq!(
        heap.handle_generation(bridge),
        Some(HandleGeneration::Young)
    );
    assert_eq!(heap.handle_generation(target), Some(HandleGeneration::Old));
}

#[test]
fn old_mark_remains_active_across_complete_young_cycle() {
    let heap = zgc_heap();
    let mut old_nlab = Nlab::new();
    let old_chain: Vec<_> = (0..200)
        .map(|_| {
            let handle = publish_object(&heap, &mut old_nlab, 1);
            heap.promote_to_old(handle).unwrap();
            handle
        })
        .collect();
    for pair in old_chain.windows(2) {
        heap.set_property(
            pair[0],
            PropertyKey::from_name_id(1),
            value::encode_object_handle(pair[1]) as u64,
        )
        .unwrap();
    }
    for _ in 0..8 {
        let handle = publish_object(&heap, &mut old_nlab, 8_000);
        heap.promote_to_old(handle).unwrap();
    }
    let mut young_nlab = Nlab::new();
    let young = publish_object(&heap, &mut young_nlab, 1);
    let collector = GenerationalZgc::new(Arc::clone(&heap), 2, 512).unwrap();

    let GcSafepointAction::PublishRoots { epoch } = collector.safepoint_action() else {
        panic!("old pressure must start an old root handshake");
    };
    collector
        .at_safepoint(Some(RootSnapshot::new(
            epoch,
            vec![value::encode_object_handle(old_chain[0])],
            Vec::new(),
            Vec::new(),
        )))
        .unwrap();

    collector.observe_allocation(1024 * 1024, Duration::from_nanos(1));
    loop {
        match collector.safepoint_action() {
            GcSafepointAction::PublishRoots { epoch } => {
                collector
                    .at_safepoint(Some(RootSnapshot::new(
                        epoch,
                        vec![value::encode_object_handle(young)],
                        Vec::new(),
                        Vec::new(),
                    )))
                    .unwrap();
                break;
            }
            GcSafepointAction::FlushBarriers | GcSafepointAction::FinishCycle => {
                collector.at_safepoint(None).unwrap();
            }
            GcSafepointAction::Idle => std::thread::yield_now(),
            GcSafepointAction::Assist { .. } => {
                collector.at_safepoint(None).unwrap();
            }
        }
    }

    let first = drive_automatic_cycle(&collector, &[value::encode_object_handle(young)]);
    assert_eq!(first.stats.cycle_kind, wjsm_gc::CycleKind::Young);
    let mut young_cycles = 1;
    loop {
        let report = drive_automatic_cycle(&collector, &[value::encode_object_handle(young)]);
        if report.stats.cycle_kind == wjsm_gc::CycleKind::ZgcCycle {
            assert_eq!(report.stats.marked, old_chain.len() + 1);
            break;
        }
        assert_eq!(report.stats.cycle_kind, wjsm_gc::CycleKind::Young);
        young_cycles += 1;
    }
    assert!(young_cycles >= 1);
}

#[test]
fn full_barrier_ring_executes_bounded_mutator_assist_without_losing_record() {
    let layout = Arc::new(ManagedHeapLayout::new(1024 * 1024, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let barrier = Arc::new(ZgcBarrierSet::new(Arc::clone(&handles), memory.clone(), 1));
    let heap = Arc::new(
        HeapAccessV2::with_handles(
            memory,
            layout,
            handles,
            HeapBarrier::Zgc(Arc::clone(&barrier)),
        )
        .unwrap(),
    );
    let mut nlab = Nlab::new();
    let first = publish_object(&heap, &mut nlab, 0);
    let assisted = publish_object(&heap, &mut nlab, 0);
    let collector = GenerationalZgc::new(Arc::clone(&heap), 2, 16).unwrap();
    collector.observe_allocation(1024 * 1024, Duration::from_nanos(1));
    let GcSafepointAction::PublishRoots { epoch } = collector.safepoint_action() else {
        panic!("allocation pressure must start a young cycle");
    };
    collector
        .at_safepoint(Some(RootSnapshot::new(
            epoch,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )))
        .unwrap();

    barrier
        .record(BarrierRecord::Mark(value::encode_object_handle(first)))
        .unwrap();
    barrier
        .record(BarrierRecord::Mark(value::encode_object_handle(assisted)))
        .unwrap();
    assert!(
        heap.is_marked_handle(assisted, HandleGeneration::Young)
            .unwrap()
    );

    let report = drive_automatic_cycle(&collector, &[]);
    assert_eq!(report.stats.marked, 2);
}
