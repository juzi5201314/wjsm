//! Concurrent old generation mark spanning multiple young cycles.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::heap::{HandleGeneration, HandleId};
use crate::runtime_gc::RootSnapshot;

use super::barrier::BarrierEpoch;
use super::young::YoungController;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OldPhase {
    Idle,
    ConcurrentMark,
    PauseMarkEnd,
    ConcurrentSelectRelocationSet,
    ConcurrentRelocate,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OldReport {
    pub marked: usize,
    pub young_cycles_spanned: usize,
    pub promotion_frontier: usize,
    pub young_to_old_roots: usize,
    pub pause_ns_max: u64,
    pub old_live_bytes: u64,
    pub mark_work_bytes: u64,
}

#[derive(Clone, Debug)]
struct OldObjectMeta {
    references: Vec<Option<HandleId>>,
    bytes: u64,
    generation: HandleGeneration,
}

pub struct OldController {
    phase: Mutex<OldPhase>,
    epoch: Mutex<BarrierEpoch>,
    pending: Mutex<VecDeque<HandleId>>,
    marked: Mutex<BTreeSet<HandleId>>,
    objects: Mutex<BTreeMap<HandleId, OldObjectMeta>>,
    promotion_frontier: Mutex<Vec<HandleId>>,
    report: Mutex<OldReport>,
    active: AtomicBool,
    young_cycles: AtomicUsize,
    inflight: AtomicUsize,
    mark_epoch: AtomicU64,
}

impl OldController {
    pub fn new() -> Self {
        Self {
            phase: Mutex::new(OldPhase::Idle),
            epoch: Mutex::new(BarrierEpoch::IDLE),
            pending: Mutex::new(VecDeque::new()),
            marked: Mutex::new(BTreeSet::new()),
            objects: Mutex::new(BTreeMap::new()),
            promotion_frontier: Mutex::new(Vec::new()),
            report: Mutex::new(OldReport::default()),
            active: AtomicBool::new(false),
            young_cycles: AtomicUsize::new(0),
            inflight: AtomicUsize::new(0),
            mark_epoch: AtomicU64::new(0),
        }
    }

    pub fn phase(&self) -> OldPhase {
        *self.phase.lock()
    }

    pub fn report(&self) -> OldReport {
        *self.report.lock()
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    pub fn register_object(
        &self,
        handle: HandleId,
        generation: HandleGeneration,
        references: impl IntoIterator<Item = Option<HandleId>>,
        bytes: u64,
    ) {
        self.objects.lock().insert(
            handle,
            OldObjectMeta {
                references: references.into_iter().collect(),
                bytes,
                generation,
            },
        );
    }

    /// old mark 由 young mark-start 协调启动（可跨多个 young cycle）。
    pub fn coordinate_from_young_mark_start(
        &self,
        young: &YoungController,
        roots: &RootSnapshot,
        start_if_idle: bool,
    ) {
        let mut epoch = self.epoch.lock();
        if !self.active.load(Ordering::SeqCst) {
            if !start_if_idle {
                return;
            }
            *epoch = epoch.flip_old();
            // keep young marking flags independent
            let young_epoch = young.epoch();
            epoch.young_mark = young_epoch.young_mark;
            epoch.remembered = young_epoch.remembered;
            epoch.young_marking = young_epoch.young_marking;
            self.active.store(true, Ordering::SeqCst);
            self.mark_epoch.fetch_add(1, Ordering::SeqCst);
            *self.report.lock() = OldReport::default();
            self.marked.lock().clear();
            let mut pending = self.pending.lock();
            pending.clear();
            pending.extend(roots.handles().iter().copied().map(HandleId::new));
            // seed old objects currently known as old
            for (handle, meta) in self.objects.lock().iter() {
                if meta.generation == HandleGeneration::Old {
                    pending.push_back(*handle);
                }
            }
            *self.phase.lock() = OldPhase::ConcurrentMark;
        } else {
            self.young_cycles.fetch_add(1, Ordering::SeqCst);
            self.report.lock().young_cycles_spanned += 1;
            // absorb promotion frontier published by young
            let frontier = std::mem::take(&mut *self.promotion_frontier.lock());
            self.report.lock().promotion_frontier += frontier.len();
            let mut pending = self.pending.lock();
            for handle in frontier {
                pending.push_back(handle);
            }
        }
        // young→old roots: young roots that point into old generation
        let mut young_to_old = 0usize;
        let objects = self.objects.lock();
        for handle in roots.handles().iter().copied().map(HandleId::new) {
            if objects
                .get(&handle)
                .is_some_and(|meta| meta.generation == HandleGeneration::Old)
            {
                young_to_old += 1;
                self.pending.lock().push_back(handle);
            }
        }
        self.report.lock().young_to_old_roots += young_to_old;
    }

    pub fn note_promoted(&self, handle: HandleId) {
        if self.active.load(Ordering::SeqCst) {
            self.promotion_frontier.lock().push(handle);
            if let Some(meta) = self.objects.lock().get_mut(&handle) {
                meta.generation = HandleGeneration::Old;
            }
        }
    }

    pub fn concurrent_mark_step(&self, work_budget: usize) -> bool {
        if self.phase() != OldPhase::ConcurrentMark {
            return false;
        }
        let budget = work_budget.max(1);
        let mut worked = 0u64;
        for _ in 0..budget {
            let Some(handle) = self.pending.lock().pop_front() else {
                break;
            };
            if !self.marked.lock().insert(handle) {
                continue;
            }
            let meta = self.objects.lock().get(&handle).cloned();
            let Some(meta) = meta else {
                continue;
            };
            if meta.generation != HandleGeneration::Old {
                continue;
            }
            self.report.lock().marked += 1;
            self.report.lock().old_live_bytes += meta.bytes;
            self.report.lock().mark_work_bytes += meta.bytes;
            worked += meta.bytes;
            for reference in meta.references.into_iter().flatten() {
                if !self.marked.lock().contains(&reference) {
                    self.pending.lock().push_back(reference);
                }
            }
        }
        let _ = worked;
        !self.pending.lock().is_empty()
    }

    /// PauseOldMarkEnd：只完成 buffer/termination handshake，不做 old 全量工作。
    pub fn pause_mark_end(&self) -> Duration {
        let started = Instant::now();
        // residual drain only — fixed tiny budget, never full old heap.
        let _ = self.concurrent_mark_step(16);
        if self.pending.lock().is_empty() && self.inflight.load(Ordering::SeqCst) == 0 {
            *self.phase.lock() = OldPhase::ConcurrentSelectRelocationSet;
            let mut epoch = self.epoch.lock();
            *epoch = epoch.end_old();
            self.active.store(false, Ordering::SeqCst);
        }
        let elapsed = started.elapsed();
        let ns = elapsed.as_nanos() as u64;
        let mut report = self.report.lock();
        report.pause_ns_max = report.pause_ns_max.max(ns);
        elapsed
    }

    pub fn is_marked(&self, handle: HandleId) -> bool {
        self.marked.lock().contains(&handle)
    }

    pub fn mark_work_normalized_by_old_live(&self) -> Option<f64> {
        let report = self.report();
        if report.old_live_bytes == 0 {
            return None;
        }
        Some(report.mark_work_bytes as f64 / report.old_live_bytes as f64)
    }
}

impl Default for OldController {
    fn default() -> Self {
        Self::new()
    }
}
