use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::control::{GcRuntimeV2, RootSnapshot};

/// mutator 只发布 handle roots；Wasm Store/Caller/WasmEnv 永远留在 mutator 侧。
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

    pub fn publish_roots(&self, roots: impl IntoIterator<Item = u32>) -> RootSnapshot {
        let epoch = self.runtime.requested_epoch();
        let snapshot = RootSnapshot::new(epoch, roots.into_iter().collect());
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
