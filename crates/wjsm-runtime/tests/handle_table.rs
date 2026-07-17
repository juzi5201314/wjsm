#![cfg(feature = "managed-heap-v2")]

use wjsm_runtime::{
    HANDLE_ENTRY_BYTES, HANDLE_REGION_BYTES, HandleGeneration, HandleId, HandleState,
    HandleTableV2, ManagedHeapLayout,
};

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn handle_table_reserves_32_gib_without_committing_every_entry() {
    let layout = ManagedHeapLayout::new(4 * GIB, 128 * 1024).unwrap();
    let table = HandleTableV2::new(layout.clone()).unwrap();

    assert_eq!(table.reserved_bytes(), HANDLE_REGION_BYTES);
    assert_eq!(table.committed_bytes(), 0);
    assert_eq!(layout.control_base(), HANDLE_REGION_BYTES);
    assert_eq!(layout.control_end(), layout.object_heap_base());
    assert_eq!(
        layout.object_heap_end(),
        layout.object_heap_base() + 4 * GIB
    );
}

#[test]
fn handle_table_addresses_high_u32_handle_and_resolves_single_live_entry() {
    let layout = ManagedHeapLayout::new(16 * GIB, 64 * 1024).unwrap();
    let table = HandleTableV2::new(layout.clone()).unwrap();
    let handle = HandleId::new(u32::MAX);
    let address = layout.object_heap_base() + 64;

    table
        .publish(handle, address, HandleGeneration::Young)
        .unwrap();
    assert_eq!(table.committed_bytes(), table.block_bytes());
    assert_eq!(
        HandleTableV2::entry_address(handle),
        HANDLE_REGION_BYTES - HANDLE_ENTRY_BYTES
    );
    assert_eq!(
        layout.object_heap_end(),
        layout.object_heap_base() + 16 * GIB
    );
    let entry = table.resolve(handle).unwrap();
    assert_eq!(entry.address(), address);
    assert_eq!(entry.generation(), HandleGeneration::Young);
    assert_eq!(entry.state(), HandleState::StableYoung);
}

#[test]
fn handle_table_promotes_relocates_retires_and_reuses_after_epoch_grace() {
    let layout = ManagedHeapLayout::new(4 * GIB, 64 * 1024).unwrap();
    let table = HandleTableV2::new(layout).unwrap();
    let participant = table.register_participant();
    let handle = table.allocate_handle().unwrap();
    table
        .publish(
            handle,
            table.layout().object_heap_base() + 8,
            HandleGeneration::Young,
        )
        .unwrap();
    table.promote(handle).unwrap();
    table.begin_relocation(handle).unwrap();
    assert_eq!(
        table.resolve(handle).unwrap().state(),
        HandleState::RelocatingOld
    );
    table
        .complete_relocation(handle, table.layout().object_heap_base() + 16)
        .unwrap();
    assert_eq!(
        table.resolve(handle).unwrap().generation(),
        HandleGeneration::Old
    );

    participant.enter();
    table.retire(handle).unwrap();
    assert_eq!(table.reclaim_quarantine(), 0);
    participant.exit();
    table.advance_epoch();
    assert_eq!(table.reclaim_quarantine(), 1);
    assert_eq!(table.allocate_handle().unwrap(), handle);
}
