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
