use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use wjsm_ir::value;

use super::collector_context::CollectorContext;
use super::mutator::MutatorContext;

/// V2 collector/mutator 协调的共享 owner；不持有 heap 或 collector 算法。
pub struct GcRuntimeV2 {
    requested_epoch: AtomicU64,
    next_mutator_id: AtomicU32,
    next_collector_id: AtomicU32,
    active_mutators: AtomicUsize,
    active_collectors: AtomicUsize,
}

impl GcRuntimeV2 {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            requested_epoch: AtomicU64::new(0),
            next_mutator_id: AtomicU32::new(0),
            next_collector_id: AtomicU32::new(0),
            active_mutators: AtomicUsize::new(0),
            active_collectors: AtomicUsize::new(0),
        })
    }

    pub fn register_mutator(self: &Arc<Self>) -> MutatorContext {
        let participant_id = self.next_mutator_id.fetch_add(1, Ordering::Relaxed);
        self.active_mutators.fetch_add(1, Ordering::SeqCst);
        MutatorContext::new(Arc::clone(self), participant_id)
    }

    pub fn register_collector(self: &Arc<Self>) -> CollectorContext {
        let collector_id = self.next_collector_id.fetch_add(1, Ordering::Relaxed);
        self.active_collectors.fetch_add(1, Ordering::SeqCst);
        CollectorContext::new(Arc::clone(self), collector_id)
    }

    /// 请求下一个 root snapshot epoch；不锁住任何 collector 算法状态。
    pub fn request_root_snapshot(&self) -> u64 {
        self.requested_epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn requested_epoch(&self) -> u64 {
        self.requested_epoch.load(Ordering::SeqCst)
    }

    pub fn active_mutators(&self) -> usize {
        self.active_mutators.load(Ordering::SeqCst)
    }

    pub fn active_collectors(&self) -> usize {
        self.active_collectors.load(Ordering::SeqCst)
    }

    pub(crate) fn mutator_dropped(&self) {
        self.active_mutators.fetch_sub(1, Ordering::SeqCst);
    }

    pub(crate) fn collector_dropped(&self) {
        self.active_collectors.fetch_sub(1, Ordering::SeqCst);
    }
}

/// collector 消费的 immutable encoded-value 图；worker 不持有 mutator/host 状态。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GcEdge {
    pub owner: i64,
    pub target: i64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GcEphemeron {
    pub owner: i64,
    pub key: i64,
    pub value: i64,
}

#[derive(Clone)]
pub struct RootSnapshot {
    epoch: u64,
    roots: Arc<[i64]>,
    strong_edges: Arc<[GcEdge]>,
    ephemerons: Arc<[GcEphemeron]>,
}

impl RootSnapshot {
    pub fn new(
        epoch: u64,
        roots: Vec<i64>,
        mut strong_edges: Vec<GcEdge>,
        mut ephemerons: Vec<GcEphemeron>,
    ) -> Self {
        strong_edges.sort_unstable();
        ephemerons.sort_unstable();
        Self {
            epoch,
            roots: roots.into(),
            strong_edges: strong_edges.into(),
            ephemerons: ephemerons.into(),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn with_epoch(&self, epoch: u64) -> Self {
        Self {
            epoch,
            roots: Arc::clone(&self.roots),
            strong_edges: Arc::clone(&self.strong_edges),
            ephemerons: Arc::clone(&self.ephemerons),
        }
    }

    pub fn roots(&self) -> &[i64] {
        &self.roots
    }

    pub fn root_handles(&self) -> impl Iterator<Item = u32> + '_ {
        self.roots
            .iter()
            .copied()
            .filter(|encoded| value::is_handle_backed_reference(*encoded))
            .map(value::decode_handle)
    }

    pub fn strong_edges(&self) -> &[GcEdge] {
        &self.strong_edges
    }

    pub fn ephemerons(&self) -> &[GcEphemeron] {
        &self.ephemerons
    }
}
