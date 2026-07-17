use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use super::collector_context::CollectorContext;
use super::mutator::MutatorContext;

/// V2 collector/mutator 协调的共享 owner；本任务不持有 heap、Store 或 collector 算法。
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

/// collector 只能消费不可变、handle-only 的 root snapshot，不能回持 mutator Store。
#[derive(Clone)]
pub struct RootSnapshot {
    epoch: u64,
    handles: Arc<[u32]>,
}

impl RootSnapshot {
    pub(crate) fn new(epoch: u64, handles: Vec<u32>) -> Self {
        Self {
            epoch,
            handles: handles.into(),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn handles(&self) -> &[u32] {
        &self.handles
    }
}
