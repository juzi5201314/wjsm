#![cfg(feature = "managed-heap-v2")]

use wjsm_runtime::{
    AllocationClass, ManagedAllocator, ManagedHeapLayout, Nlab, ObjectRef, PAGE_GRANULE_BYTES,
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
fn object_map_and_double_bitmap_stream_live_objects_without_raw_pointers() {
    let allocator = allocator();
    let mut nlab = Nlab::new();
    let first = allocator.allocate(&mut nlab, 16).unwrap();
    let second = allocator.allocate(&mut nlab, 32).unwrap();

    allocator.mark_current(first.object()).unwrap();
    allocator.mark_previous(second.object()).unwrap();
    let objects: Vec<ObjectRef> = allocator.objects_in_page(first.page()).collect();

    assert_eq!(objects, vec![first.object(), second.object()]);
    assert!(allocator.is_marked_current(first.object()).unwrap());
    assert!(allocator.is_marked_previous(second.object()).unwrap());
    assert!(!allocator.is_marked_current(second.object()).unwrap());
}

#[test]
fn relocation_reserve_isolated_and_released_ranges_coalesce() {
    let allocator = allocator();
    let reserve = allocator.reserve_relocation(4).unwrap();
    let mut nlab = Nlab::new();
    let large = allocator.allocate(&mut nlab, 2 * MIB).unwrap();

    assert!(!reserve.pages().overlaps(large.pages()));
    allocator.release_dedicated(&large).unwrap();
    allocator.release_relocation(reserve).unwrap();
    assert_eq!(allocator.free_pages(), allocator.total_pages());
}
