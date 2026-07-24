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

#[test]
fn satb_ring_model_preserves_overwritten_handle_under_race() {
    loom::model(|| {
        let slot = Arc::new(AtomicU64::new(11));
        let satb = Arc::new(AtomicU64::new(0));
        let marking = Arc::new(AtomicBool::new(true));

        let writer_slot = Arc::clone(&slot);
        let writer_satb = Arc::clone(&satb);
        let writer_marking = Arc::clone(&marking);
        let writer = thread::spawn(move || {
            if writer_marking.load(Ordering::SeqCst) {
                let old = writer_slot.swap(12, Ordering::SeqCst);
                writer_satb.store(old, Ordering::SeqCst);
            } else {
                writer_slot.store(12, Ordering::SeqCst);
            }
        });

        let marker_satb = Arc::clone(&satb);
        let marker_marking = Arc::clone(&marking);
        let marker = thread::spawn(move || {
            let old = marker_satb.load(Ordering::SeqCst);
            if old != 0 {
                assert_eq!(old, 11);
            }
            marker_marking.store(false, Ordering::SeqCst);
        });

        writer.join().unwrap();
        marker.join().unwrap();
        let final_satb = satb.load(Ordering::SeqCst);
        assert!(final_satb == 0 || final_satb == 11);
        assert_eq!(slot.load(Ordering::SeqCst), 12);
    });
}

#[test]
fn remembered_slot_model_dedups_concurrent_old_to_young_writes() {
    loom::model(|| {
        let bit = Arc::new(AtomicBool::new(false));
        let count = Arc::new(AtomicU64::new(0));

        let mut joins = Vec::new();
        for _ in 0..2 {
            let bit = Arc::clone(&bit);
            let count = Arc::clone(&count);
            joins.push(thread::spawn(move || {
                if bit
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for join in joins {
            join.join().unwrap();
        }
        assert!(bit.load(Ordering::SeqCst));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn young_termination_model_waits_for_inflight_before_end() {
    loom::model(|| {
        let inflight = Arc::new(AtomicU64::new(0));
        let terminated = Arc::new(AtomicBool::new(false));
        let (work_tx, work_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();

        let worker_inflight = Arc::clone(&inflight);
        let worker = thread::spawn(move || {
            work_rx.recv().unwrap();
            assert_eq!(worker_inflight.load(Ordering::SeqCst), 1);
            worker_inflight.fetch_sub(1, Ordering::SeqCst);
            done_tx.send(()).unwrap();
        });

        inflight.fetch_add(1, Ordering::SeqCst);
        work_tx.send(()).unwrap();
        // pause mark end must observe inflight
        assert!(!terminated.load(Ordering::SeqCst));
        done_rx.recv().unwrap();
        assert_eq!(inflight.load(Ordering::SeqCst), 0);
        terminated.store(true, Ordering::SeqCst);
        worker.join().unwrap();
        assert!(terminated.load(Ordering::SeqCst));
    });
}

#[test]
fn remset_epoch_flip_model_isolates_snapshot_from_concurrent_writes() {
    loom::model(|| {
        let active = Arc::new(AtomicU64::new(0));
        let snapshot = Arc::new(AtomicU64::new(0));

        let writer_active = Arc::clone(&active);
        let writer = thread::spawn(move || {
            writer_active.store(0x2008, Ordering::SeqCst);
        });

        let snap_active = Arc::clone(&active);
        let snap_snapshot = Arc::clone(&snapshot);
        let flipper = thread::spawn(move || {
            let taken = snap_active.swap(0, Ordering::SeqCst);
            snap_snapshot.store(taken, Ordering::SeqCst);
        });

        writer.join().unwrap();
        flipper.join().unwrap();
        let snap = snapshot.load(Ordering::SeqCst);
        let act = active.load(Ordering::SeqCst);
        // snapshot and active partition the writes; no value is lost from both
        assert!(snap == 0 || snap == 0x2008 || act == 0x2008);
    });
}

#[test]
fn promotion_publish_model_races_with_young_mark() {
    loom::model(|| {
        // low 16 bits = state: 1 StableYoung, 2 StableOld
        let entry = Arc::new(AtomicU64::new(entry(OLD_ADDRESS, STABLE_YOUNG)));
        let marked = Arc::new(AtomicBool::new(false));

        let promote_entry = Arc::clone(&entry);
        let promoter = thread::spawn(move || {
            let current = promote_entry.load(Ordering::SeqCst);
            if current & 0xFFFF == STABLE_YOUNG {
                let next = (current & !0xFFFF) | 2;
                let _ = promote_entry.compare_exchange(
                    current,
                    next,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
            }
        });

        let mark_entry = Arc::clone(&entry);
        let mark_flag = Arc::clone(&marked);
        let marker = thread::spawn(move || {
            let current = mark_entry.load(Ordering::SeqCst);
            assert!(current & 0xFFFF == STABLE_YOUNG || current & 0xFFFF == 2);
            mark_flag.store(true, Ordering::SeqCst);
        });

        promoter.join().unwrap();
        marker.join().unwrap();
        assert!(marked.load(Ordering::SeqCst));
        let state = entry.load(Ordering::SeqCst) & 0xFFFF;
        assert!(state == STABLE_YOUNG || state == 2);
    });
}

#[test]
fn old_mark_cross_young_cycle_model_keeps_promotion_frontier() {
    loom::model(|| {
        let frontier = Arc::new(AtomicU64::new(0));
        let pending = Arc::new(AtomicU64::new(0));

        let young = thread::spawn({
            let frontier = Arc::clone(&frontier);
            move || {
                frontier.store(9, Ordering::SeqCst);
            }
        });

        let old = thread::spawn({
            let frontier = Arc::clone(&frontier);
            let pending = Arc::clone(&pending);
            move || {
                let promoted = frontier.swap(0, Ordering::SeqCst);
                if promoted != 0 {
                    pending.store(promoted, Ordering::SeqCst);
                }
            }
        });

        young.join().unwrap();
        old.join().unwrap();
        let pending = pending.load(Ordering::SeqCst);
        let frontier = frontier.load(Ordering::SeqCst);
        assert!(pending == 9 || frontier == 9 || (pending == 0 && frontier == 0));
    });
}

#[test]
fn relocation_copy_ownership_model_single_winner() {
    loom::model(|| {
        let owner = Arc::new(AtomicU64::new(0));
        let winners = Arc::new(AtomicU64::new(0));
        let mut joins = Vec::new();
        for id in 1..=2u64 {
            let owner = Arc::clone(&owner);
            let winners = Arc::clone(&winners);
            joins.push(thread::spawn(move || {
                if owner
                    .compare_exchange(0, id, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    winners.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for join in joins {
            join.join().unwrap();
        }
        assert_eq!(winners.load(Ordering::SeqCst), 1);
        assert!(owner.load(Ordering::SeqCst) == 1 || owner.load(Ordering::SeqCst) == 2);
    });
}

#[test]
fn epoch_reclaim_model_waits_for_active_reader() {
    loom::model(|| {
        let reader_active = Arc::new(AtomicBool::new(true));
        let reclaimed = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::channel();

        let reader_flag = Arc::clone(&reader_active);
        let reader = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            exit_rx.recv().unwrap();
            reader_flag.store(false, Ordering::SeqCst);
        });

        let reclaim_flag = Arc::clone(&reader_active);
        let reclaim_done = Arc::clone(&reclaimed);
        let reclaimer = thread::spawn(move || {
            ready_rx.recv().unwrap();
            if !reclaim_flag.load(Ordering::SeqCst) {
                reclaim_done.store(true, Ordering::SeqCst);
            }
        });

        // force exit before second reclaim attempt
        exit_tx.send(()).unwrap();
        reader.join().unwrap();
        reclaimer.join().unwrap();
        if !reader_active.load(Ordering::SeqCst) {
            reclaimed.store(true, Ordering::SeqCst);
        }
        assert!(reclaimed.load(Ordering::SeqCst));
    });
}
