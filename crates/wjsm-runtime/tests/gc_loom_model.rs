#![cfg(feature = "managed-heap-v2")]

use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use loom::sync::mpsc;
use loom::thread;

const STABLE_YOUNG: u64 = 1;
const RETIRED: u64 = 6;
const OLD_ADDRESS: u64 = 0x10_0000;
const NEW_ADDRESS: u64 = 0x20_0000;

fn entry(address: u64, state: u64) -> u64 {
    (address << 16) | state
}

#[test]
fn handle_aba_model_keeps_retired_slot_quarantined_while_reader_is_active() {
    loom::model(|| {
        let handle = Arc::new(AtomicU64::new(entry(OLD_ADDRESS, STABLE_YOUNG)));
        let reader_active = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(AtomicU64::new(0));
        let (reader_ready_tx, reader_ready_rx) = mpsc::channel();
        let (collector_checked_tx, collector_checked_rx) = mpsc::channel();
        let (reader_exited_tx, reader_exited_rx) = mpsc::channel();

        let reader_handle = Arc::clone(&handle);
        let reader_active_flag = Arc::clone(&reader_active);
        let reader_observed = Arc::clone(&observed);
        let reader = thread::spawn(move || {
            reader_active_flag.store(true, Ordering::SeqCst);
            reader_observed.store(reader_handle.load(Ordering::SeqCst), Ordering::SeqCst);
            reader_ready_tx.send(()).unwrap();
            collector_checked_rx.recv().unwrap();
            reader_active_flag.store(false, Ordering::SeqCst);
            reader_exited_tx.send(()).unwrap();
        });

        let collector_handle = Arc::clone(&handle);
        let collector_active_flag = Arc::clone(&reader_active);
        let collector = thread::spawn(move || {
            reader_ready_rx.recv().unwrap();
            collector_handle.store(entry(OLD_ADDRESS, RETIRED), Ordering::SeqCst);
            assert!(collector_active_flag.load(Ordering::SeqCst));
            assert_eq!(
                collector_handle.load(Ordering::SeqCst),
                entry(OLD_ADDRESS, RETIRED)
            );
            collector_checked_tx.send(()).unwrap();
            reader_exited_rx.recv().unwrap();
            assert!(!collector_active_flag.load(Ordering::SeqCst));
            collector_handle.store(entry(NEW_ADDRESS, STABLE_YOUNG), Ordering::SeqCst);
        });

        reader.join().unwrap();
        collector.join().unwrap();
        assert_eq!(
            observed.load(Ordering::SeqCst),
            entry(OLD_ADDRESS, STABLE_YOUNG)
        );
        assert_eq!(
            handle.load(Ordering::SeqCst),
            entry(NEW_ADDRESS, STABLE_YOUNG)
        );
    });
}

#[test]
fn inflight_termination_model_drains_admitted_packet_before_worker_exit() {
    loom::model(|| {
        let accepting = Arc::new(AtomicBool::new(true));
        let inflight = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let (packet_tx, packet_rx) = mpsc::channel();
        let (admitted_tx, admitted_rx) = mpsc::channel();
        let (start_tx, start_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();

        let producer_accepting = Arc::clone(&accepting);
        let producer_inflight = Arc::clone(&inflight);
        let producer = thread::spawn(move || {
            assert!(producer_accepting.load(Ordering::SeqCst));
            producer_inflight.fetch_add(1, Ordering::SeqCst);
            packet_tx.send(()).unwrap();
            admitted_tx.send(()).unwrap();
        });

        let worker_accepting = Arc::clone(&accepting);
        let worker_inflight = Arc::clone(&inflight);
        let worker_processed = Arc::clone(&processed);
        let worker = thread::spawn(move || {
            packet_rx.recv().unwrap();
            start_rx.recv().unwrap();
            assert!(!worker_accepting.load(Ordering::SeqCst));
            assert_eq!(worker_inflight.load(Ordering::SeqCst), 1);
            worker_processed.fetch_add(1, Ordering::SeqCst);
            worker_inflight.fetch_sub(1, Ordering::SeqCst);
            done_tx.send(()).unwrap();
        });

        admitted_rx.recv().unwrap();
        accepting.store(false, Ordering::SeqCst);
        start_tx.send(()).unwrap();
        done_rx.recv().unwrap();
        assert_eq!(inflight.load(Ordering::SeqCst), 0);
        assert_eq!(processed.load(Ordering::SeqCst), 1);
        producer.join().unwrap();
        worker.join().unwrap();
    });
}
