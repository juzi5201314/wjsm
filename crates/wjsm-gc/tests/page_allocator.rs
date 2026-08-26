use wjsm_gc::{
    AllocationClass, GrowableHeapMemory, HandleGeneration, HandleTableV2, ManagedAllocator,
    ManagedHeapLayout, Nlab, ObjectRef, PAGE_GRANULE_BYTES,
};

const MIB: u64 = 1024 * 1024;

fn allocator() -> ManagedAllocator {
    let layout = ManagedHeapLayout::new(256 * MIB, 64 * 1024).unwrap();
    ManagedAllocator::new(layout).unwrap()
}

#[test]
fn nlab_allocates_heap_relative_objects_without_reentering_global_allocator() {
    let allocator = allocator();
    let mut nlab = Nlab::new();
    assert_eq!(allocator.committed_bytes(), 0);
    assert_eq!(allocator.allocated_bytes(), 0);

    let first = allocator.allocate(&mut nlab, 24).unwrap();
    let second = allocator.allocate(&mut nlab, 40).unwrap();

    assert_eq!(first.class(), AllocationClass::Small);
    assert_eq!(second.class(), AllocationClass::Small);
    assert_eq!(second.object().offset(), first.object().offset() + 24);
    assert_eq!(nlab.refills(), 1);
    assert_eq!(allocator.object_count(first.page()), 2);
    assert_eq!(allocator.allocated_bytes(), 64);
    assert_eq!(allocator.committed_bytes(), PAGE_GRANULE_BYTES);
}

#[test]
fn allocator_selects_page_classes_and_contiguous_ranges() {
    let allocator = allocator();
    let mut nlab = Nlab::new();

    let medium = allocator.allocate(&mut nlab, 64 * 1024).unwrap();
    let large = allocator.allocate(&mut nlab, 2 * MIB).unwrap();
    let humongous = allocator.allocate(&mut nlab, 40 * MIB).unwrap();

    assert_eq!(medium.class(), AllocationClass::Medium);
    assert_eq!(large.class(), AllocationClass::Large);
    assert_eq!(humongous.class(), AllocationClass::Humongous);
    assert!(humongous.pages().len() > 1);
    assert!(allocator.pages_are_contiguous(humongous.pages()));
}

#[test]
fn object_map_and_generation_bitmaps_stream_live_objects_without_size_table() {
    let allocator = allocator();
    let mut nlab = Nlab::new();
    let first = allocator.allocate(&mut nlab, 16).unwrap();
    let second = allocator.allocate(&mut nlab, 32).unwrap();

    assert!(
        allocator
            .try_mark(first.object(), first.bytes(), HandleGeneration::Young)
            .unwrap()
    );
    assert!(
        !allocator
            .try_mark(first.object(), first.bytes(), HandleGeneration::Young)
            .unwrap()
    );
    assert!(
        allocator
            .try_mark(second.object(), second.bytes(), HandleGeneration::Old)
            .unwrap()
    );
    let objects: Vec<ObjectRef> = allocator.objects_in_page(first.page()).collect();

    assert_eq!(objects, vec![first.object(), second.object()]);
    assert!(
        allocator
            .is_marked(first.object(), HandleGeneration::Young)
            .unwrap()
    );
    assert!(
        allocator
            .is_marked(second.object(), HandleGeneration::Old)
            .unwrap()
    );
    assert!(
        !allocator
            .is_marked(second.object(), HandleGeneration::Young)
            .unwrap()
    );
    let stats = allocator.page_stats();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].allocated_bytes, 48);
    assert_eq!(stats[0].young_live_bytes, 16);
    assert_eq!(stats[0].old_live_bytes, 32);
    assert_eq!(stats[0].object_count, 2);
    assert!(!stats[0].dedicated);
}

#[test]
fn native_tlab_zeroes_range_and_materializes_small_objects() {
    let allocator = allocator();
    let layout = allocator.layout().clone();
    let memory = wjsm_gc::TestHeapMemory::for_layout(&layout);
    memory
        .grow_to(layout.object_heap_base() + PAGE_GRANULE_BYTES)
        .unwrap();
    let table = HandleTableV2::new(layout).unwrap();
    let handles = table.reserve_range(4).unwrap();
    let mut reservation = allocator.reserve_native_tlab(handles).unwrap();
    let object_start = reservation.object_start();
    memory
        .store_word(wjsm_gc::HeapAddress::new(object_start), u64::MAX)
        .unwrap();
    reservation.zero_range(&memory).unwrap();
    assert!(reservation.is_zeroed());
    assert_eq!(
        memory
            .load_word(wjsm_gc::HeapAddress::new(object_start))
            .unwrap(),
        0
    );
    memory
        .store_word(wjsm_gc::HeapAddress::new(object_start), 3)
        .unwrap();
    reservation
        .materialize_native_tlab(
            object_start + 24,
            reservation.handle_start() + 1,
            |_, _| Ok(24),
            &allocator,
            Some(HandleGeneration::Young),
        )
        .unwrap();
    assert_eq!(allocator.allocated_bytes(), 24);
    assert_eq!(allocator.page_stats()[0].object_count, 1);
    assert_eq!(allocator.page_stats()[0].young_live_bytes, 24);
}

#[test]
fn relocation_nlab_allocates_reserved_pages_and_returns_unused_tail() {
    let allocator = allocator();
    let mut relocation = allocator.reserve_relocation(4).unwrap();
    let relocated = allocator.allocate_relocation(&mut relocation, 24).unwrap();
    assert_eq!(relocation.remaining_pages(), 3);

    let mut nlab = Nlab::new();
    let large = allocator.allocate(&mut nlab, 2 * MIB).unwrap();
    assert!(!relocated.pages().overlaps(large.pages()));

    allocator.finish_relocation(relocation).unwrap();
    allocator.release_dedicated(&large).unwrap();
    allocator
        .reclaim_object(relocated.object(), relocated.bytes())
        .unwrap();
    assert_eq!(allocator.free_pages(), allocator.total_pages());
}
