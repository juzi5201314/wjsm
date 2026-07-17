#![cfg(feature = "managed-heap-v2")]

use std::sync::{Arc, Barrier};
use std::thread;

use wjsm_runtime::{HandleGeneration, HandleState, HandleTableV2, ManagedHeapLayout};

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn handle_quarantine_keeps_retired_slot_unreusable_until_active_reader_exits() {
    let layout = ManagedHeapLayout::new(4 * GIB, 64 * 1024).unwrap();
    let table = Arc::new(HandleTableV2::new(layout).unwrap());
    let handle = table.allocate_handle().unwrap();
    table
        .publish(
            handle,
            table.layout().object_heap_base() + 8,
            HandleGeneration::Young,
        )
        .unwrap();

    let participant = table.register_participant();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let reader_entered = Arc::clone(&entered);
    let reader_release = Arc::clone(&release);
    let reader = thread::spawn(move || {
        participant.enter();
        reader_entered.wait();
        reader_release.wait();
        participant.exit();
    });

    entered.wait();
    table.retire(handle).unwrap();
    assert_eq!(table.reclaim_quarantine(), 0);
    assert_eq!(table.resolve(handle), None);
    release.wait();
    reader.join().unwrap();

    table.advance_epoch();
    assert_eq!(table.reclaim_quarantine(), 1);
    assert_eq!(table.allocate_handle().unwrap(), handle);
    assert_eq!(table.resolve(handle), None);
    assert_eq!(
        table
            .publish(
                handle,
                table.layout().object_heap_base() + 16,
                HandleGeneration::Old,
            )
            .unwrap(),
        ()
    );
    assert_eq!(
        table.resolve(handle).unwrap().state(),
        HandleState::StableOld
    );
}
