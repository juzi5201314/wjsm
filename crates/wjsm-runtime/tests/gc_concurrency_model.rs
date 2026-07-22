use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use wjsm_runtime::{
    GcPacketKind, GcWorkPacket, GcWorkerPool, HandleGeneration, HandleState, HandleTableV2,
    ManagedHeapLayout,
};

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

#[test]
fn worker_pool_processes_concurrent_producers_without_packet_loss() {
    const PRODUCERS: usize = 4;
    const PACKETS_PER_PRODUCER: usize = 64;

    let completed = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&completed);
    let pool = Arc::new(
        GcWorkerPool::new(4, 512, move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap(),
    );
    pool.wait_for_parked_workers(4);

    let mut producers = Vec::new();
    for producer in 0..PRODUCERS {
        let producer_pool = Arc::clone(&pool);
        producers.push(thread::spawn(move || {
            for offset in 0..PACKETS_PER_PRODUCER {
                producer_pool
                    .submit(GcWorkPacket::new(
                        GcPacketKind::BitmapWordRange,
                        (producer * PACKETS_PER_PRODUCER + offset) as u64,
                        1,
                        11,
                    ))
                    .unwrap();
            }
        }));
    }
    for producer in producers {
        producer.join().unwrap();
    }
    pool.wait_for_idle();

    assert_eq!(
        completed.load(Ordering::SeqCst),
        PRODUCERS * PACKETS_PER_PRODUCER
    );
    assert_eq!(
        pool.stats().completed,
        (PRODUCERS * PACKETS_PER_PRODUCER) as u64
    );
    pool.shutdown();
}
