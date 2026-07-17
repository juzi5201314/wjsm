#![cfg(feature = "managed-heap-v2")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use parking_lot::Mutex;

use wjsm_runtime::{GcPacketKind, GcWorkPacket, GcWorkerPool, WorkerPoolError};

fn packet(id: u64) -> GcWorkPacket {
    GcWorkPacket::new(GcPacketKind::PageRange, id, 1, 7)
}

#[test]
fn gc_worker_pool_reuses_packet_slab_and_rejects_work_after_ordered_shutdown() {
    let executed = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&executed);
    let pool = GcWorkerPool::new(2, 8, move |_, packet| {
        observed.lock().push(packet.start());
    })
    .unwrap();

    pool.wait_for_parked_workers(2);
    for id in 0..8 {
        pool.submit(packet(id)).unwrap();
    }
    pool.wait_for_idle();
    let warmup_allocations = pool.packet_slab_allocations();
    assert_eq!(warmup_allocations, 8);

    for id in 8..16 {
        pool.submit(packet(id)).unwrap();
    }
    pool.shutdown();
    assert_eq!(pool.packet_slab_allocations(), warmup_allocations);
    assert_eq!(executed.lock().len(), 16);
    assert_eq!(pool.live_workers(), 0);
    assert_eq!(pool.submit(packet(16)), Err(WorkerPoolError::ShuttingDown));
}

#[test]
fn gc_worker_pool_wakes_peer_while_owner_packet_is_blocked() {
    let first_packet = Arc::new(AtomicBool::new(true));
    let (blocked_tx, blocked_rx) = mpsc::channel();
    let (peer_tx, peer_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let first_packet_for_handler = Arc::clone(&first_packet);
    let release_rx_for_handler = Arc::clone(&release_rx);
    let pool = GcWorkerPool::new(2, 16, move |worker, _| {
        if first_packet_for_handler.swap(false, Ordering::SeqCst) {
            blocked_tx.send(worker).unwrap();
            release_rx_for_handler.lock().recv().unwrap();
        } else {
            peer_tx.send(worker).unwrap();
        }
    })
    .unwrap();

    pool.wait_for_parked_workers(2);
    for id in 0..16 {
        pool.submit(packet(id)).unwrap();
    }
    let blocked_worker = blocked_rx.recv().unwrap();
    let peer_worker = peer_rx.recv().unwrap();
    assert_ne!(peer_worker, blocked_worker);
    release_tx.send(()).unwrap();
    pool.wait_for_idle();
    assert!(pool.stats().parks >= 2);
    pool.shutdown();
}
