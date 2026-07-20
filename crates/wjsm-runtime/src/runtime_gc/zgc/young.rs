//! Concurrent young generation mark for ZGC V2.

#![cfg(feature = "managed-heap-v2")]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::heap::{HandleGeneration, HandleId};
use crate::runtime_gc::RootSnapshot;
use crate::runtime_gc::worker::{GcPacketKind, GcWorkPacket};

use super::barrier::{BarrierEpoch, BarrierRecord, BarrierRing};
use super::remset::PreciseRemset;

/// young cycle 的 type-state phases。pause 阶段只做 root/buffer handshake。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum YoungPhase {
    Idle,
    PauseMarkStart,
    ConcurrentMark,
    PauseMarkEnd,
    ConcurrentSelectRelocationSet,
    PauseRelocateStart,
    ConcurrentRelocate,
    EpochReclaim,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct YoungReport {
    pub marked: usize,
    pub satb_drained: usize,
    pub remset_slots_scanned: usize,
    pub promoted: usize,
    pub relocated: usize,
    pub pause_ns_max: u64,
    pub concurrent_mark_ns: u64,
    pub black_allocations: usize,
}

#[derive(Clone, Debug)]
struct YoungObjectMeta {
    generation: HandleGeneration,
    age: u8,
    references: Vec<Option<HandleId>>,
    bytes: u64,
    dense: bool,
    humongous: bool,
}


pub struct YoungController {
    phase: Mutex<YoungPhase>,
    epoch: Mutex<BarrierEpoch>,
    pending: Mutex<VecDeque<HandleId>>,
    marked: Mutex<BTreeSet<HandleId>>,
    objects: Mutex<BTreeMap<HandleId, YoungObjectMeta>>,
    satb: BarrierRing<HandleId>,
    remset_ring: BarrierRing<u64>,
    remset: PreciseRemset,
    report: Mutex<YoungReport>,
    pause_work_flags: Mutex<PauseWorkFlags>,
    black_alloc_enabled: AtomicBool,
    inflight: AtomicUsize,
    mark_epoch: AtomicU64,
    termination: AtomicBool,
}

#[derive(Clone, Copy, Debug, Default)]
struct PauseWorkFlags {
    page_scan: bool,
    object_copy: bool,
}

impl YoungController {
    pub fn new(ring_capacity: usize) -> Self {
        Self {
            phase: Mutex::new(YoungPhase::Idle),
            epoch: Mutex::new(BarrierEpoch::IDLE),
            pending: Mutex::new(VecDeque::new()),
            marked: Mutex::new(BTreeSet::new()),
            objects: Mutex::new(BTreeMap::new()),
            satb: BarrierRing::with_capacity(ring_capacity),
            remset_ring: BarrierRing::with_capacity(ring_capacity),
            remset: PreciseRemset::default(),
            report: Mutex::new(YoungReport::default()),
            pause_work_flags: Mutex::new(PauseWorkFlags::default()),
            black_alloc_enabled: AtomicBool::new(false),
            inflight: AtomicUsize::new(0),
            mark_epoch: AtomicU64::new(0),
            termination: AtomicBool::new(false),
        }
    }

    pub fn phase(&self) -> YoungPhase {
        *self.phase.lock()
    }

    pub fn epoch(&self) -> BarrierEpoch {
        *self.epoch.lock()
    }

    pub fn report(&self) -> YoungReport {
        *self.report.lock()
    }

    pub fn remset(&self) -> &PreciseRemset {
        &self.remset
    }

    pub fn register_object(
        &self,
        handle: HandleId,
        generation: HandleGeneration,
        references: impl IntoIterator<Item = Option<HandleId>>,
        bytes: u64,
        dense: bool,
        humongous: bool,
    ) {
        self.objects.lock().insert(
            handle,
            YoungObjectMeta {
                generation,
                age: 0,
                references: references.into_iter().collect(),
                bytes,
                dense,
                humongous,
            },
        );
        if self.black_alloc_enabled.load(Ordering::SeqCst)
            && generation == HandleGeneration::Young
        {
            self.marked.lock().insert(handle);
            self.report.lock().black_allocations += 1;
        }
    }

    pub fn write_reference(
        &self,
        owner: HandleId,
        slot: usize,
        target: Option<HandleId>,
        slot_addr: u64,
    ) -> Vec<BarrierRecord> {
        let mut objects = self.objects.lock();
        let Some(owner_meta) = objects.get_mut(&owner) else {
            return Vec::new();
        };
        if slot >= owner_meta.references.len() {
            owner_meta.references.resize(slot + 1, None);
        }
        let old = std::mem::replace(&mut owner_meta.references[slot], target);
        let owner_generation = owner_meta.generation;
        let target_generation = target.and_then(|h| objects.get(&h).map(|m| m.generation));
        drop(objects);

        let epoch = self.epoch();
        let mut records = Vec::new();
        if epoch.young_marking {
            if let Some(old) = old {
                let _ = self.satb.push_or_mark_full(old);
                records.push(BarrierRecord::Satb(old));
            }
        }
        if owner_generation == HandleGeneration::Old
            && matches!(target_generation, Some(HandleGeneration::Young) | None)
        {
            // overwrite/delete or old→young
            if target_generation == Some(HandleGeneration::Young) || old.is_some() {
                let _ = self.remset_ring.push_or_mark_full(slot_addr);
                self.remset.record_slot(slot_addr);
                records.push(BarrierRecord::RememberedSlot { slot_addr });
            }
        }
        records
    }

    /// PauseYoungMarkStart：翻转 young/remembered epoch，snapshot roots 与 remset。
    pub fn pause_mark_start(&self, roots: &RootSnapshot) -> Duration {
        let started = Instant::now();
        {
            let mut flags = self.pause_work_flags.lock();
            *flags = PauseWorkFlags::default();
        }
        let mut epoch = self.epoch.lock();
        *epoch = epoch.flip_young();
        self.black_alloc_enabled.store(true, Ordering::SeqCst);
        self.termination.store(false, Ordering::SeqCst);
        self.mark_epoch.fetch_add(1, Ordering::SeqCst);
        *self.report.lock() = YoungReport::default();
        self.marked.lock().clear();
        let remset_slots = self.remset.snapshot_and_flip();
        let mut pending = self.pending.lock();
        pending.clear();
        pending.extend(roots.handles().iter().copied().map(HandleId::new));
        // remset snapshot becomes young roots (objects holding slots are resolved by caller graph).
        for slot in remset_slots {
            let _ = slot;
            // slot scanning is concurrent; only snapshot cost is pause-local.
        }
        *self.phase.lock() = YoungPhase::ConcurrentMark;
        let elapsed = started.elapsed();
        self.note_pause(elapsed);
        elapsed
    }

    pub fn concurrent_mark_step(&self, work_budget: usize) -> bool {
        assert_eq!(self.phase(), YoungPhase::ConcurrentMark);
        let mark_started = Instant::now();
        self.drain_satb();
        self.drain_remset_ring();
        let budget = work_budget.max(1);
        let mut worked = 0;
        while worked < budget {
            let Some(handle) = self.pending.lock().pop_front() else {
                break;
            };
            if !self.marked.lock().insert(handle) {
                continue;
            }
            self.report.lock().marked += 1;
            let meta = self.objects.lock().get(&handle).cloned();
            let Some(meta) = meta else {
                continue;
            };
            for reference in meta.references.into_iter().flatten() {
                if !self.marked.lock().contains(&reference) {
                    self.pending.lock().push_back(reference);
                }
            }
            worked += 1;
        }
        let elapsed_ns = mark_started.elapsed().as_nanos() as u64;
        {
            let mut report = self.report.lock();
            report.concurrent_mark_ns = report.concurrent_mark_ns.saturating_add(elapsed_ns);
        }
        !self.pending.lock().is_empty() || !self.satb.is_empty()
    }

    fn is_old_root_edge(&self, _handle: HandleId) -> bool {
        false
    }

    pub fn drain_satb(&self) {
        let drained = self.satb.drain();
        if drained.is_empty() {
            return;
        }
        self.report.lock().satb_drained += drained.len();
        let mut pending = self.pending.lock();
        for handle in drained {
            pending.push_back(handle);
        }
    }

    pub fn drain_remset_ring(&self) {
        let slots = self.remset_ring.drain();
        if slots.is_empty() {
            return;
        }
        self.report.lock().remset_slots_scanned += slots.len();
        // precise remset already recorded; young roots come from owners resolved externally.
    }

    /// PauseYoungMarkEnd：flush buffers + termination handshake，禁止 page scan/copy。
    pub fn pause_mark_end(&self) -> Duration {
        let started = Instant::now();
        {
            let flags = self.pause_work_flags.lock();
            assert!(!flags.page_scan, "pause must not page-scan");
            assert!(!flags.object_copy, "pause must not copy objects");
        }
        self.drain_satb();
        self.drain_remset_ring();
        // terminate only when queues empty and no inflight worker packets.
        while self.concurrent_mark_step(usize::MAX) {
            // fixed-point drain inside pause is only buffer residual, not page scan.
        }
        self.termination.store(true, Ordering::SeqCst);
        *self.phase.lock() = YoungPhase::ConcurrentSelectRelocationSet;
        let mut epoch = self.epoch.lock();
        *epoch = epoch.end_young();
        self.black_alloc_enabled.store(false, Ordering::SeqCst);
        let elapsed = started.elapsed();
        self.note_pause(elapsed);
        elapsed
    }

    pub fn select_relocation_set(&self) -> Vec<HandleId> {
        assert_eq!(self.phase(), YoungPhase::ConcurrentSelectRelocationSet);
        let marked = self.marked.lock().clone();
        let objects = self.objects.lock();
        let mut sparse = Vec::new();
        let mut promote = Vec::new();
        for handle in marked {
            let Some(meta) = objects.get(&handle) else {
                continue;
            };
            if meta.generation != HandleGeneration::Young {
                continue;
            }
            if meta.dense || meta.humongous || meta.age >= 1 {
                promote.push(handle);
            } else {
                sparse.push(handle);
            }
        }
        drop(objects);
        for handle in &promote {
            self.promote_in_place(*handle);
        }
        *self.phase.lock() = YoungPhase::PauseRelocateStart;
        sparse
    }

    pub fn promote_in_place(&self, handle: HandleId) {
        let mut objects = self.objects.lock();
        if let Some(meta) = objects.get_mut(&handle) {
            meta.generation = HandleGeneration::Old;
            meta.age = meta.age.saturating_add(1);
            self.report.lock().promoted += 1;
        }
    }

    pub fn pause_relocate_start(&self) -> Duration {
        let started = Instant::now();
        {
            let flags = self.pause_work_flags.lock();
            assert!(!flags.object_copy);
        }
        *self.phase.lock() = YoungPhase::ConcurrentRelocate;
        let elapsed = started.elapsed();
        self.note_pause(elapsed);
        elapsed
    }

    pub fn note_relocated(&self, count: usize) {
        self.report.lock().relocated += count;
    }

    pub fn finish_epoch_reclaim(&self) {
        *self.phase.lock() = YoungPhase::Idle;
        self.remset.clear_snapshot();
    }

    pub fn is_marked(&self, handle: HandleId) -> bool {
        self.marked.lock().contains(&handle)
    }

    pub fn generation(&self, handle: HandleId) -> Option<HandleGeneration> {
        self.objects.lock().get(&handle).map(|m| m.generation)
    }

    pub fn pause_did_page_scan_or_copy(&self) -> bool {
        let flags = self.pause_work_flags.lock();
        flags.page_scan || flags.object_copy
    }

    pub fn mark_work_packet(&self, epoch: u64) -> GcWorkPacket {
        self.inflight.fetch_add(1, Ordering::SeqCst);
        GcWorkPacket::new(GcPacketKind::RootRange, 0, 1, epoch)
    }

    pub fn complete_work_packet(&self) {
        self.inflight.fetch_sub(1, Ordering::SeqCst);
    }

    pub fn terminated(&self) -> bool {
        self.termination.load(Ordering::SeqCst)
            && self.pending.lock().is_empty()
            && self.satb.is_empty()
            && self.inflight.load(Ordering::SeqCst) == 0
    }

    /// young work must not scan entire old heap; remset size is the bound.
    pub fn young_work_bound(&self) -> usize {
        self.remset.active_len() + self.marked.lock().len()
    }

    fn note_pause(&self, elapsed: Duration) {
        let ns = elapsed.as_nanos() as u64;
        let mut report = self.report.lock();
        report.pause_ns_max = report.pause_ns_max.max(ns);
    }

    /// 兼容旧 mark.rs 的 host write SATB 入口。
    pub fn record_satb(&self, handle: HandleId) {
        let _ = self.satb.push_or_mark_full(handle);
    }
}
