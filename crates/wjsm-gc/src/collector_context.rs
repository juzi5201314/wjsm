use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::control::{GcRuntimeV2, RootSnapshot};

/// collector context 是 Store-free、Caller-free 的线程安全 view。
pub struct CollectorContext {
    runtime: Arc<GcRuntimeV2>,
    collector_id: u32,
    observed_epoch: AtomicU64,
}

impl CollectorContext {
    pub(crate) fn new(runtime: Arc<GcRuntimeV2>, collector_id: u32) -> Self {
        Self {
            runtime,
            collector_id,
            observed_epoch: AtomicU64::new(u64::MAX),
        }
    }

    pub fn collector_id(&self) -> u32 {
        self.collector_id
    }

    /// 仅当 snapshot epoch 与上次观察不同才返回 true。
    pub fn observe_roots(&self, snapshot: &RootSnapshot) -> bool {
        self.observed_epoch.swap(snapshot.epoch(), Ordering::SeqCst) != snapshot.epoch()
    }

    pub fn observed_epoch(&self) -> u64 {
        self.observed_epoch.load(Ordering::SeqCst)
    }
}

impl Drop for CollectorContext {
    fn drop(&mut self) {
        self.runtime.collector_dropped();
    }
}
