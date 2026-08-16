use std::sync::Arc;

use wjsm_gc::{
    BarrierEpoch, BarrierRecord, HandleGeneration, HandleTableV2, HeaderLayout, HeapAccessV2,
    HeapBarrier, ManagedHeapLayout, Nlab, PAGE_GRANULE_BYTES, PROTO_NULL_SENTINEL,
    RelocationDescriptor, TestHeapMemory, ZgcBarrierSet,
};
use wjsm_ir::constants;

fn allocate(heap: &HeapAccessV2<TestHeapMemory>, bytes: u64) -> u64 {
    heap.allocate(&mut Nlab::new(), bytes)
        .unwrap()
        .object()
        .offset()
}

#[test]
fn property_growth_consumes_exact_relocation_bytes() {
    const HEAP_BYTES: u64 = 1024 * 1024;
    const PROPERTY_COUNT: u32 = 512;

    let layout = Arc::new(ManagedHeapLayout::new(HEAP_BYTES, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let heap = HeapAccessV2::with_handles(memory, layout, handles, wjsm_gc::HeapBarrier::Disabled)
        .unwrap();
    let handle = heap.allocate_handle().unwrap();
    let object = allocate(&heap, u64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    heap.publish_object(handle, object, PROTO_NULL_SENTINEL, 0)
        .unwrap();
    assert_eq!(heap.object_handle_at(object).unwrap(), handle);
    assert_eq!(heap.object_age_at(object).unwrap(), 0);

    for key in 1..=PROPERTY_COUNT {
        heap.set_property(handle, key, u64::from(key)).unwrap();
    }

    assert_eq!(
        heap.get_property_slot(handle, PROPERTY_COUNT)
            .unwrap()
            .unwrap()
            .value,
        u64::from(PROPERTY_COUNT)
    );
    assert!(
        heap.used_bytes() < 128 * 1024,
        "property relocation consumed {} bytes",
        heap.used_bytes()
    );
}

#[test]
fn growth_transfers_active_mark_to_destination() {
    let layout = Arc::new(ManagedHeapLayout::new(1024 * 1024, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let barriers = Arc::new(ZgcBarrierSet::new(Arc::clone(&handles), memory.clone(), 8));
    barriers.set_epoch(BarrierEpoch {
        young_marking: true,
        ..BarrierEpoch::IDLE
    });
    let heap = HeapAccessV2::with_handles(
        memory,
        layout,
        handles,
        HeapBarrier::Zgc(Arc::clone(&barriers)),
    )
    .unwrap();
    let handle = heap.allocate_handle().unwrap();
    let object = allocate(&heap, u64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    heap.publish_object(handle, object, PROTO_NULL_SENTINEL, 0)
        .unwrap();
    assert!(
        heap.is_marked_handle(handle, HandleGeneration::Young)
            .unwrap()
    );

    heap.grow_object_capacity(handle, 4).unwrap();

    assert!(
        heap.is_marked_handle(handle, HandleGeneration::Young)
            .unwrap()
    );
    assert_eq!(
        heap.object_handle_at(heap.resolve_handle(handle).unwrap())
            .unwrap(),
        handle
    );
}

#[test]
fn scan_references_reads_prototype_and_full_payload_capacity() {
    let layout = Arc::new(ManagedHeapLayout::new(1024 * 1024, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let heap = HeapAccessV2::with_handles(memory, layout, handles, wjsm_gc::HeapBarrier::Disabled)
        .unwrap();
    let mut nlab = Nlab::new();
    let prototype = heap.allocate_handle().unwrap();
    let prototype_object = heap
        .allocate(&mut nlab, u64::from(constants::HEAP_OBJECT_HEADER_SIZE))
        .unwrap()
        .object()
        .offset();
    heap.publish_object(prototype, prototype_object, PROTO_NULL_SENTINEL, 0)
        .unwrap();
    let target = heap.allocate_handle().unwrap();
    let target_object = heap
        .allocate(&mut nlab, u64::from(constants::HEAP_OBJECT_HEADER_SIZE))
        .unwrap()
        .object()
        .offset();
    heap.publish_object(target, target_object, PROTO_NULL_SENTINEL, 0)
        .unwrap();
    let owner = heap.allocate_handle().unwrap();
    let owner_object = heap
        .allocate(
            &mut nlab,
            u64::from(constants::HEAP_OBJECT_HEADER_SIZE + 2 * 8),
        )
        .unwrap()
        .object()
        .offset();
    heap.publish_object(owner, owner_object, prototype, 2)
        .unwrap();
    heap.set_property(
        owner,
        1,
        wjsm_ir::value::encode_object_handle(target) as u64,
    )
    .unwrap();

    let mut handles = Vec::new();
    heap.scan_references(owner, |encoded| {
        if wjsm_ir::value::is_handle_backed_reference(encoded) {
            handles.push(wjsm_ir::value::decode_handle(encoded));
        }
    })
    .unwrap();
    assert_eq!(handles, [prototype, target]);
}

#[test]
fn heap_access_rejects_mismatched_memory_and_handle_layouts() {
    let memory_layout = ManagedHeapLayout::new(1024 * 1024, 64 * 1024).unwrap();
    let handle_layout = Arc::new(ManagedHeapLayout::new(2 * 1024 * 1024, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&memory_layout);
    let handles = Arc::new(HandleTableV2::new(handle_layout.as_ref().clone()).unwrap());

    let error = HeapAccessV2::with_handles(
        memory,
        handle_layout,
        handles,
        wjsm_gc::HeapBarrier::Disabled,
    )
    .err()
    .expect("mismatched logical ranges must be rejected");
    assert!(error.to_string().contains("does not match managed layout"));
}

#[test]
fn snapshot_restore_rebuilds_page_objects_from_stable_handles() {
    const HEAP_BYTES: u64 = 1024 * 1024;
    let layout = Arc::new(ManagedHeapLayout::new(HEAP_BYTES, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let source = HeapAccessV2::with_handles(
        memory,
        Arc::clone(&layout),
        handles,
        wjsm_gc::HeapBarrier::Disabled,
    )
    .unwrap();
    let mut nlab = Nlab::new();
    let first = source.allocate_handle().unwrap();
    let first_object = source
        .allocate(&mut nlab, u64::from(constants::HEAP_OBJECT_HEADER_SIZE))
        .unwrap()
        .object()
        .offset();
    source
        .publish_object(first, first_object, PROTO_NULL_SENTINEL, 0)
        .unwrap();
    let second = source.allocate_handle().unwrap();
    let second_object = source
        .allocate(
            &mut nlab,
            u64::from(constants::HEAP_OBJECT_HEADER_SIZE + constants::HEAP_OBJECT_VALUE_SLOT_SIZE),
        )
        .unwrap()
        .object()
        .offset();
    source
        .publish_object(second, second_object, PROTO_NULL_SENTINEL, 1)
        .unwrap();
    let object_bytes = source.capture_object_region().unwrap();
    let (entries, next_handle) = source.capture_handles().unwrap();

    let restored_memory = TestHeapMemory::for_layout(&layout);
    let restored_handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let restored = HeapAccessV2::with_handles(
        restored_memory,
        layout,
        restored_handles,
        wjsm_gc::HeapBarrier::Disabled,
    )
    .unwrap();
    restored.restore_object_region(&object_bytes).unwrap();
    restored.restore_handles(&entries, next_handle).unwrap();
    restored.restore_page_metadata(&entries).unwrap();

    assert_eq!(restored.resolve_handle(first).unwrap(), first_object);
    assert_eq!(restored.resolve_handle(second).unwrap(), second_object);
    let stats = restored.page_stats();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].object_count, 2);
    assert_eq!(
        stats[0].allocated_bytes,
        u64::from(constants::HEAP_OBJECT_HEADER_SIZE * 2 + constants::HEAP_OBJECT_VALUE_SLOT_SIZE)
    );
}

#[test]
fn zgc_heap_access_assists_relocating_handle_before_returning_address() {
    let layout = Arc::new(ManagedHeapLayout::new(1024 * 1024, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let barriers = Arc::new(ZgcBarrierSet::new(Arc::clone(&handles), memory.clone(), 8));
    let heap = HeapAccessV2::with_handles(
        memory,
        layout,
        Arc::clone(&handles),
        HeapBarrier::Zgc(Arc::clone(&barriers)),
    )
    .unwrap();
    let mut nlab = Nlab::new();
    let bytes = u64::from(constants::HEAP_OBJECT_HEADER_SIZE);
    let source = heap.allocate(&mut nlab, bytes).unwrap().object().offset();
    let destination = heap.allocate(&mut nlab, bytes).unwrap().object().offset();
    let handle = heap.allocate_handle().unwrap();
    heap.publish_object(handle, source, PROTO_NULL_SENTINEL, 0)
        .unwrap();
    barriers
        .relocator()
        .install_descriptor(RelocationDescriptor::new(
            wjsm_gc::HandleId::new(handle),
            source,
            destination,
            bytes,
            HandleGeneration::Young,
            HeaderLayout::OBJECT,
        ));
    handles
        .begin_relocation(wjsm_gc::HandleId::new(handle))
        .unwrap();

    assert_eq!(heap.resolve_handle(handle).unwrap(), destination);
    assert_eq!(heap.object_handle_at(destination).unwrap(), handle);
}

#[test]
fn zgc_store_reference_records_mark_and_old_to_young_slot() {
    let layout = Arc::new(ManagedHeapLayout::new(1024 * 1024, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let memory_view = memory.clone();
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let barriers = Arc::new(ZgcBarrierSet::new(Arc::clone(&handles), memory.clone(), 8));
    let heap = HeapAccessV2::with_handles(
        memory,
        layout,
        handles,
        HeapBarrier::Zgc(Arc::clone(&barriers)),
    )
    .unwrap();
    let mut nlab = Nlab::new();
    let owner = heap.allocate_handle().unwrap();
    let owner_object = heap
        .allocate(
            &mut nlab,
            wjsm_gc::heap_access::object_payload_bytes(1).unwrap(),
        )
        .unwrap()
        .object()
        .offset();
    heap.publish_object(owner, owner_object, PROTO_NULL_SENTINEL, 1)
        .unwrap();
    heap.promote_to_old(owner).unwrap();
    assert!(heap.try_mark_handle(owner).unwrap());
    let target = heap.allocate_handle().unwrap();
    let target_object = heap
        .allocate(
            &mut nlab,
            wjsm_gc::heap_access::object_payload_bytes(0).unwrap(),
        )
        .unwrap()
        .object()
        .offset();
    heap.publish_object(target, target_object, PROTO_NULL_SENTINEL, 0)
        .unwrap();
    barriers.set_epoch(BarrierEpoch {
        young_marking: true,
        ..BarrierEpoch::IDLE
    });
    let slot = wjsm_gc::heap_access::value_slot_address(owner_object, 0).unwrap();
    let target_value = wjsm_ir::value::encode_object_handle(target);
    heap.store_reference(owner, slot, target_value as u64)
        .unwrap();

    let mut records = Vec::new();
    barriers.drain_records(|record| records.push(record));
    assert!(records.contains(&BarrierRecord::Mark(target_value)));
    assert!(records.contains(&BarrierRecord::RememberedSlot { slot_addr: slot }));
    assert_eq!(
        wjsm_ir::value::strip_gc_color(
            memory_view
                .load_word(wjsm_gc::HeapAddress::new(slot))
                .unwrap() as i64
        ),
        target_value
    );
}
#[test]
fn relocated_allocation_is_reused_only_after_epoch_grace_period() {
    let layout = Arc::new(ManagedHeapLayout::new(1024 * 1024, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let epoch = handles.epoch();
    let heap = HeapAccessV2::with_handles(memory, layout, handles, wjsm_gc::HeapBarrier::Disabled)
        .unwrap();
    assert!(Arc::ptr_eq(&epoch, &heap.epoch()));
    let handle = heap.allocate_handle().unwrap();
    let object = allocate(&heap, u64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    heap.publish_object(handle, object, PROTO_NULL_SENTINEL, 0)
        .unwrap();

    let reader = heap.register_epoch_participant();
    reader.enter();
    heap.grow_object_capacity(handle, 4).unwrap();
    let free_before_reclaim = heap.free_bytes();
    assert_eq!(heap.advance_epoch_and_reclaim().unwrap(), (0, 0));
    assert_eq!(heap.free_bytes(), free_before_reclaim);

    reader.exit();
    assert_eq!(heap.advance_epoch_and_reclaim().unwrap(), (0, 1));
    assert_eq!(heap.free_bytes(), free_before_reclaim + PAGE_GRANULE_BYTES);
}

#[test]
fn reused_prototype_handle_invalidates_proto_generation() {
    let layout = Arc::new(ManagedHeapLayout::new(1024 * 1024, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let heap = HeapAccessV2::with_handles(memory, layout, handles, wjsm_gc::HeapBarrier::Disabled)
        .unwrap();
    let prototype = heap.allocate_handle().unwrap();
    let receiver = heap.allocate_handle().unwrap();
    let header_bytes = u64::from(constants::HEAP_OBJECT_HEADER_SIZE);

    let prototype_object = allocate(&heap, header_bytes);
    heap.publish_object(prototype, prototype_object, PROTO_NULL_SENTINEL, 0)
        .unwrap();
    let receiver_object = allocate(&heap, header_bytes);
    heap.publish_object(receiver, receiver_object, PROTO_NULL_SENTINEL, 0)
        .unwrap();
    heap.set_prototype(receiver, prototype).unwrap();
    let generation = heap.shapes().proto_generation();

    // 模拟 GC 清扫旧 receiver 与 prototype；回收顺序让 prototype handle 先复用。
    heap.retire_handle(receiver).unwrap();
    heap.retire_handle(prototype).unwrap();
    heap.advance_epoch_and_reclaim().unwrap();
    heap.advance_epoch_and_reclaim().unwrap();
    assert_eq!(heap.allocate_handle().unwrap(), prototype);

    let replacement_object = allocate(&heap, header_bytes);
    heap.publish_object(prototype, replacement_object, PROTO_NULL_SENTINEL, 0)
        .unwrap();
    // NativeRuntime 的对象分配在 publish 后调用 set_prototype；该绑定必须使旧 IC 失效。
    heap.set_prototype(prototype, PROTO_NULL_SENTINEL).unwrap();
    assert_ne!(heap.shapes().proto_generation(), generation);
}

#[test]
fn own_data_property_index_covers_data_accessor_dictionary_array_missing() {
    const HEAP_BYTES: u64 = 1024 * 1024;

    let layout = Arc::new(ManagedHeapLayout::new(HEAP_BYTES, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let heap = HeapAccessV2::with_handles(memory, layout, handles, wjsm_gc::HeapBarrier::Disabled)
        .unwrap();

    fn publish(heap: &HeapAccessV2<TestHeapMemory>) -> u32 {
        let handle = heap.allocate_handle().unwrap();
        let object = allocate(heap, u64::from(constants::HEAP_OBJECT_HEADER_SIZE));
        heap.publish_object(handle, object, PROTO_NULL_SENTINEL, 0)
            .unwrap();
        handle
    }

    // 1. 普通对象 + 自有数据属性 → Some((shape_id, value_index))
    let handle = publish(&heap);
    heap.set_property(handle, 1, 42).unwrap();
    let (shape_id, index) = heap
        .own_data_property_index(handle, 1)
        .unwrap()
        .expect("own data property must resolve");
    assert_eq!(shape_id, heap.shape_id(handle).unwrap());
    assert_eq!(
        heap.get_property_slot(handle, 1).unwrap().unwrap().value,
        42
    );
    assert!(index < constants::DICTIONARY_THRESHOLD);

    // 2. accessor 属性 → None（值槽里是 getter/setter，快路径不可直读）
    let accessor_handle = publish(&heap);
    heap.define_accessor_property(accessor_handle, 2, 1, 2)
        .unwrap();
    assert!(
        heap.own_data_property_index(accessor_handle, 2)
            .unwrap()
            .is_none()
    );

    // 3. 超过字典阈值的对象 → None（字典 shape 独占，IC 永不回填）
    let dict_handle = publish(&heap);
    for key in 1..=constants::DICTIONARY_THRESHOLD + 1 {
        heap.set_property(dict_handle, key, u64::from(key)).unwrap();
    }
    assert!(
        heap.own_data_property_index(dict_handle, 1)
            .unwrap()
            .is_none()
    );

    // 4. 数组 → None（数组没有 shape，命名属性走宿主侧表）
    let array_handle = heap.allocate_handle().unwrap();
    let array_object = allocate(&heap, u64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    heap.publish_array(array_handle, array_object, PROTO_NULL_SENTINEL, 0)
        .unwrap();
    assert!(
        heap.own_data_property_index(array_handle, 1)
            .unwrap()
            .is_none()
    );

    // 5. 缺失属性 → None
    assert!(heap.own_data_property_index(handle, 999).unwrap().is_none());
}
