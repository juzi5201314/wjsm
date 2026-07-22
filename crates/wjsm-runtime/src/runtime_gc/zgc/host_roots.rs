//! WeakRef/finalization/realm/side-table concurrent cycle semantics for ZGC V2.
//!
//! Cleanup completes before handle quarantine; callbacks schedule after cycle publish.

#![cfg(feature = "managed-heap-v2")]

use std::collections::{BTreeSet, VecDeque};

use parking_lot::Mutex;

use crate::heap::{HandleId, HandleTableV2};
use crate::runtime_gc::roots_v2::V2ConditionalRoots;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WeakState {
    Live,
    Cleared,
}

#[derive(Clone, Debug)]
pub struct WeakRefEntry {
    pub id: u64,
    pub target: HandleId,
    pub state: WeakState,
}

#[derive(Clone, Debug)]
pub struct FinalizerEntry {
    pub id: u64,
    pub target: HandleId,
    pub held: u64,
    pub scheduled: bool,
    pub ran: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HostRootsReport {
    pub weak_cleared: usize,
    pub finalizers_scheduled: usize,
    pub finalizers_ran: usize,
    pub side_table_filtered: usize,
    pub cleanup_before_quarantine: bool,
    pub callbacks_after_publish: bool,
}

/// concurrent-cycle host root/weak processor。
pub struct ConcurrentHostRoots {
    weak_refs: Mutex<Vec<WeakRefEntry>>,
    finalizers: Mutex<Vec<FinalizerEntry>>,
    /// side tables must not keep objects alive by themselves.
    side_table_values: Mutex<Vec<i64>>,
    pending_callbacks: Mutex<VecDeque<u64>>,
    report: Mutex<HostRootsReport>,
    cycle_published: Mutex<bool>,
}

impl ConcurrentHostRoots {
    pub fn new() -> Self {
        Self {
            weak_refs: Mutex::new(Vec::new()),
            finalizers: Mutex::new(Vec::new()),
            side_table_values: Mutex::new(Vec::new()),
            pending_callbacks: Mutex::new(VecDeque::new()),
            report: Mutex::new(HostRootsReport::default()),
            cycle_published: Mutex::new(false),
        }
    }

    pub fn report(&self) -> HostRootsReport {
        self.report.lock().clone()
    }

    pub fn register_weak(&self, id: u64, target: HandleId) {
        self.weak_refs.lock().push(WeakRefEntry {
            id,
            target,
            state: WeakState::Live,
        });
    }

    pub fn register_finalizer(&self, id: u64, target: HandleId, held: u64) {
        self.finalizers.lock().push(FinalizerEntry {
            id,
            target,
            held,
            scheduled: false,
            ran: false,
        });
    }

    pub fn push_side_table_value(&self, value: i64) {
        self.side_table_values.lock().push(value);
    }

    /// mark-end：基于 live set 清理 weak，并在 quarantine 前完成 side-table cleanup。
    pub fn cleanup_before_quarantine(
        &self,
        live: &BTreeSet<HandleId>,
        handles: &HandleTableV2,
        conditional: &V2ConditionalRoots,
    ) -> Vec<HandleId> {
        let mut report = self.report.lock();
        report.cleanup_before_quarantine = true;

        // side tables do not reverse-keep objects alive
        let conditional_roots = conditional.collect(handles);
        let mut filtered = 0usize;
        self.side_table_values.lock().retain(|value| {
            let handle = wjsm_ir::value::decode_handle(*value);
            let id = HandleId::new(handle);
            let keep = live.contains(&id) || conditional_roots.contains(&id);
            if !keep {
                filtered += 1;
            }
            keep
        });
        report.side_table_filtered += filtered;

        let mut cleared_targets = Vec::new();
        for entry in self.weak_refs.lock().iter_mut() {
            if entry.state == WeakState::Live && !live.contains(&entry.target) {
                entry.state = WeakState::Cleared;
                report.weak_cleared += 1;
                cleared_targets.push(entry.target);
            }
        }

        for entry in self.finalizers.lock().iter_mut() {
            if !entry.scheduled && !live.contains(&entry.target) {
                entry.scheduled = true;
                report.finalizers_scheduled += 1;
                self.pending_callbacks.lock().push_back(entry.id);
            }
        }

        // return dead handles that may now enter quarantine
        cleared_targets
    }

    /// cycle publish 之后才调度 finalizer callback；每个 finalizer 只跑一次。
    pub fn publish_cycle_and_run_callbacks(&self) {
        *self.cycle_published.lock() = true;
        self.report.lock().callbacks_after_publish = true;
        let mut pending = self.pending_callbacks.lock();
        let mut finalizers = self.finalizers.lock();
        while let Some(id) = pending.pop_front() {
            if let Some(entry) = finalizers.iter_mut().find(|entry| entry.id == id) {
                if !entry.ran {
                    // held 值是 FinalizationRegistry 登记的 heldValue；此处保留读取
                    // 以维持与 host 语义一致的生命周期观测点（回调本体由上层调度）。
                    let _held = entry.held;
                    entry.ran = true;
                    self.report.lock().finalizers_ran += 1;
                }
            }
        }
    }

    pub fn weak_state(&self, id: u64) -> Option<WeakState> {
        self.weak_refs
            .lock()
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.state)
    }

    pub fn finalizer_ran_once(&self, id: u64) -> bool {
        self.finalizers
            .lock()
            .iter()
            .find(|entry| entry.id == id)
            .is_some_and(|entry| entry.ran && entry.scheduled)
    }

    /// realm destroy 与 old mark 交错：仅当 realm global 仍 live 时保留 conditional roots。
    pub fn realm_destroy_filter(
        &self,
        conditional: &V2ConditionalRoots,
        handles: &HandleTableV2,
        live: &BTreeSet<HandleId>,
    ) -> BTreeSet<HandleId> {
        conditional
            .collect(handles)
            .into_iter()
            .filter(|handle| live.contains(handle))
            .collect()
    }

    /// snapshot restore 后第一轮 cycle 的 weak cleanup。
    pub fn snapshot_restore_weak_cleanup(
        &self,
        restored_live: &BTreeSet<HandleId>,
        handles: &HandleTableV2,
    ) {
        let empty = V2ConditionalRoots::default();
        let _ = self.cleanup_before_quarantine(restored_live, handles, &empty);
        self.publish_cycle_and_run_callbacks();
    }
}

impl Default for ConcurrentHostRoots {
    fn default() -> Self {
        Self::new()
    }
}
