#![cfg(feature = "managed-heap-v2")]

use std::time::Duration;

use wasmtime::{MemoryType, SharedMemory};
use wjsm_engine_config::EngineConfig;
use wjsm_runtime::{
    ConcurrentRelocator, HANDLE_REGION_BYTES, HandleGeneration, HandleId, HandleTableV2,
    HeaderLayout, ManagedHeapLayout, RelocationDescriptor, SharedHeapMemory,
};

const PAGE: u64 = 64 * 1024;

fn heap() -> (SharedHeapMemory, ManagedHeapLayout, HandleTableV2) {
    let layout = ManagedHeapLayout::new(PAGE * 16, PAGE).unwrap();
    let engine = EngineConfig::artifact().build().unwrap();
    let memory = SharedMemory::new(
        &engine,
        MemoryType::builder()
            .memory64(true)
            .shared(true)
            .min(HANDLE_REGION_BYTES / PAGE + 32)
            .max(Some(HANDLE_REGION_BYTES / PAGE + 64))
            .build()
            .unwrap(),
    )
    .unwrap();
    let heap = SharedHeapMemory::new(memory);
    heap.grow_to(layout.object_heap_base() + PAGE * 8).unwrap();
    let table = HandleTableV2::new(layout.clone()).unwrap();
    (heap, layout, table)
}

#[test]
fn copy_ownership_assist_and_destination_publish() {
    let (memory, layout, table) = heap();
    let relocator = ConcurrentRelocator::new();
    let handle = table.allocate_handle().unwrap();
    let source = layout.object_heap_base();
    let destination = layout.object_heap_base() + 256;
    table
        .publish(handle, source, HandleGeneration::Young)
        .unwrap();
    // seed source payload
    memory
        .store_word(wjsm_runtime::HeapAddress::new(source), 0xA1A2_A3A4_A5A6_A7A8)
        .unwrap();
    memory
        .store_word(wjsm_runtime::HeapAddress::new(source + 8), 0x1111)
        .unwrap();
    memory
        .store_word(wjsm_runtime::HeapAddress::new(source + 16), 0x2222)
        .unwrap();

    assert!(relocator.select_page(source / PAGE));
    let pause = relocator
        .pause_relocate_start(HandleGeneration::Young)
        .unwrap();
    assert!(pause < Duration::from_millis(1));

    let descriptor = relocator.install_descriptor(RelocationDescriptor::new(
        handle,
        source,
        destination,
        32,
        HandleGeneration::Young,
        HeaderLayout::OBJECT,
    ));

    // mutator assist claims or waits
    let addr = relocator.assist(&table, &memory, handle).unwrap();
    assert_eq!(addr, destination);
    assert!(descriptor.is_done());
    assert_eq!(table.resolve(handle).unwrap().address(), destination);
    assert_eq!(
        memory
            .load_word(wjsm_runtime::HeapAddress::new(destination + 16))
            .unwrap(),
        0x2222
    );
    assert!(relocator.report().assisted + relocator.report().relocated >= 1);
}

#[test]
fn same_slot_and_prototype_update_use_seqcst() {
    let (memory, layout, _table) = heap();
    let relocator = ConcurrentRelocator::new();
    let slot = layout.object_heap_base() + 64;
    let final_value = relocator
        .same_slot_race_safe(&memory, slot, &[(1, 10), (2, 20), (3, 30)])
        .unwrap();
    assert_eq!(final_value, 30);
    // prototype mutable word
    memory
        .store_word(wjsm_runtime::HeapAddress::new(slot), 0xDEAD)
        .unwrap();
    assert_eq!(
        memory
            .load_word(wjsm_runtime::HeapAddress::new(slot))
            .unwrap(),
        0xDEAD
    );
}

#[test]
fn source_write_rejected_while_relocating_and_young_old_mutex() {
    let (_memory, layout, table) = heap();
    let relocator = ConcurrentRelocator::new();
    let handle = table.allocate_handle().unwrap();
    table
        .publish(
            handle,
            layout.object_heap_base(),
            HandleGeneration::Young,
        )
        .unwrap();
    table.begin_relocation(handle).unwrap();
    assert!(relocator
        .reject_source_write_if_relocating(&table, handle)
        .is_err());

    relocator
        .pause_relocate_start(HandleGeneration::Young)
        .unwrap();
    assert!(relocator
        .pause_relocate_start(HandleGeneration::Old)
        .is_err());
}

#[test]
fn epoch_reclaim_after_grace_period() {
    let (_memory, layout, table) = heap();
    let relocator = ConcurrentRelocator::new();
    let handle = table.allocate_handle().unwrap();
    table
        .publish(
            handle,
            layout.object_heap_base(),
            HandleGeneration::Young,
        )
        .unwrap();
    // reader epoch participant holds quarantine
    let participant = table.register_participant();
    participant.enter();
    table.retire(handle).unwrap();
    assert_eq!(table.reclaim_quarantine(), 0);
    drop(participant);
    table.advance_epoch();
    let reclaimed = relocator.epoch_reclaim(&table);
    assert!(reclaimed >= 1);
}

#[test]
fn pause_relocate_start_has_no_copy() {
    let relocator = ConcurrentRelocator::new();
    let pause = relocator
        .pause_relocate_start(HandleGeneration::Old)
        .unwrap();
    assert!(pause < Duration::from_millis(1));
    assert_eq!(relocator.report().relocated, 0);
}
