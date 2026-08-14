use std::sync::Arc;

use wjsm_gc::{
    HandleTableV2, HeapAccessV2, ManagedHeapLayout, PROTO_NULL_SENTINEL, TestHeapMemory,
};
use wjsm_ir::constants;

#[test]
fn property_growth_consumes_exact_relocation_bytes() {
    const HEAP_BYTES: u64 = 1024 * 1024;
    const PROPERTY_COUNT: u32 = 512;

    let layout = Arc::new(ManagedHeapLayout::new(HEAP_BYTES, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let heap = HeapAccessV2::with_handles(memory, layout, handles).unwrap();
    let handle = heap.allocate_handle().unwrap();
    let (object, _) = heap
        .reserve_nlab(u64::from(constants::HEAP_OBJECT_HEADER_SIZE))
        .unwrap();
    heap.publish_object(handle, object, PROTO_NULL_SENTINEL, 0)
        .unwrap();

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
fn heap_access_rejects_mismatched_memory_and_handle_layouts() {
    let memory_layout = ManagedHeapLayout::new(1024 * 1024, 64 * 1024).unwrap();
    let handle_layout = Arc::new(ManagedHeapLayout::new(2 * 1024 * 1024, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&memory_layout);
    let handles = Arc::new(HandleTableV2::new(handle_layout.as_ref().clone()).unwrap());

    let error = HeapAccessV2::with_handles(memory, handle_layout, handles)
        .err()
        .expect("mismatched logical ranges must be rejected");
    assert!(error.to_string().contains("does not match managed layout"));
}

#[test]
fn relocated_allocation_is_reused_only_after_epoch_grace_period() {
    let layout = Arc::new(ManagedHeapLayout::new(1024 * 1024, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let heap = HeapAccessV2::with_handles(memory, layout, handles).unwrap();
    let handle = heap.allocate_handle().unwrap();
    let (object, _) = heap
        .reserve_nlab(u64::from(constants::HEAP_OBJECT_HEADER_SIZE))
        .unwrap();
    heap.publish_object(handle, object, PROTO_NULL_SENTINEL, 0)
        .unwrap();

    let reader = heap.register_epoch_participant();
    reader.enter();
    heap.grow_object_capacity(handle, 4).unwrap();
    assert_eq!(heap.free_bytes(), 0);
    assert_eq!(heap.advance_epoch_and_reclaim(), (0, 0));

    reader.exit();
    assert_eq!(heap.advance_epoch_and_reclaim(), (0, 1));
    assert_eq!(
        heap.free_bytes(),
        u64::from(constants::HEAP_OBJECT_HEADER_SIZE)
    );
}

#[test]
fn own_data_property_index_covers_data_accessor_dictionary_array_missing() {
    const HEAP_BYTES: u64 = 1024 * 1024;

    let layout = Arc::new(ManagedHeapLayout::new(HEAP_BYTES, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let heap = HeapAccessV2::with_handles(memory, layout, handles).unwrap();

    fn publish(heap: &HeapAccessV2<TestHeapMemory>) -> u32 {
        let handle = heap.allocate_handle().unwrap();
        let (object, _) = heap
            .reserve_nlab(u64::from(constants::HEAP_OBJECT_HEADER_SIZE))
            .unwrap();
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
    let (array_object, _) = heap
        .reserve_nlab(u64::from(constants::HEAP_OBJECT_HEADER_SIZE))
        .unwrap();
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
