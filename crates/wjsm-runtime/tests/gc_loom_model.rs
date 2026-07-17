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
