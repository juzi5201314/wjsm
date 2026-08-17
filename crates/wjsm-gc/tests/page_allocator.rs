use wjsm_gc::{
    AllocationClass, HandleGeneration, ManagedAllocator, ManagedHeapLayout, Nlab, ObjectRef,
    PAGE_GRANULE_BYTES,
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
