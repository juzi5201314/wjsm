use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::control::{GcRuntimeV2, RootSnapshot};

/// mutator 发布 encoded-value roots；owner context 统一持有堆与 collector 状态。
pub struct MutatorContext {
    runtime: Arc<GcRuntimeV2>,
    participant_id: u32,
    published_epoch: AtomicU64,
}

impl MutatorContext {
    pub(crate) fn new(runtime: Arc<GcRuntimeV2>, participant_id: u32) -> Self {
        Self {
            runtime,
            participant_id,
            published_epoch: AtomicU64::new(0),
        }
    }

    pub fn participant_id(&self) -> u32 {
        self.participant_id
    }

    pub fn publish_roots(&self, roots: impl IntoIterator<Item = i64>) -> RootSnapshot {
        let epoch = self.runtime.requested_epoch();
        let snapshot =
            RootSnapshot::new(epoch, roots.into_iter().collect(), Vec::new(), Vec::new());
        self.published_epoch.store(epoch, Ordering::SeqCst);
        snapshot
    }

    pub fn published_epoch(&self) -> u64 {
        self.published_epoch.load(Ordering::SeqCst)
    }
}

impl Drop for MutatorContext {
    fn drop(&mut self) {
        self.runtime.mutator_dropped();
    }
}
