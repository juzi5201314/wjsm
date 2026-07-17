use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::handle_entry::HandleId;

const INACTIVE_EPOCH: u64 = u64::MAX;

/// 追踪可能仍持有旧 handle 地址的 mutator / worker epoch。
pub(crate) struct EpochQuarantine {
    current: AtomicU64,
    next_participant: AtomicU64,
    participants: Mutex<BTreeMap<u64, Arc<AtomicU64>>>,
    retired: Mutex<Vec<RetiredHandle>>,
    reusable: Mutex<Vec<HandleId>>,
}

impl EpochQuarantine {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            current: AtomicU64::new(1),
            next_participant: AtomicU64::new(0),
            participants: Mutex::new(BTreeMap::new()),
            retired: Mutex::new(Vec::new()),
            reusable: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn register(self: &Arc<Self>) -> EpochParticipant {
        let id = self.next_participant.fetch_add(1, Ordering::SeqCst);
        let epoch = Arc::new(AtomicU64::new(INACTIVE_EPOCH));
        self.participants
            .lock()
            .expect("epoch participant lock poisoned")
            .insert(id, Arc::clone(&epoch));
        EpochParticipant {
            owner: Arc::clone(self),
            id,
            epoch,
        }
    }

    pub(crate) fn advance(&self) -> u64 {
        self.current.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub(crate) fn retire(&self, handle: HandleId) {
        let epoch = self.current.load(Ordering::SeqCst);
        self.retired
            .lock()
            .expect("epoch quarantine lock poisoned")
            .push(RetiredHandle { handle, epoch });
    }

    pub(crate) fn take_reclaimable(&self) -> Vec<HandleId> {
        let safe_epoch = self.safe_epoch();
        let mut retired = self.retired.lock().expect("epoch quarantine lock poisoned");
        let mut pending = Vec::with_capacity(retired.len());
        let mut reclaimed = Vec::new();

        for entry in retired.drain(..) {
            if entry.epoch < safe_epoch {
                reclaimed.push(entry.handle);
            } else {
                pending.push(entry);
            }
        }
        *retired = pending;
        reclaimed
    }

    pub(crate) fn make_reusable(&self, handle: HandleId) {
        self.reusable
            .lock()
            .expect("epoch reusable lock poisoned")
            .push(handle);
    }

    pub(crate) fn take_reusable(&self) -> Option<HandleId> {
        self.reusable
            .lock()
            .expect("epoch reusable lock poisoned")
            .pop()
    }

    fn safe_epoch(&self) -> u64 {
        let current = self.current.load(Ordering::SeqCst);
        self.participants
            .lock()
            .expect("epoch participant lock poisoned")
            .values()
            .map(|epoch| epoch.load(Ordering::SeqCst))
            .filter(|epoch| *epoch != INACTIVE_EPOCH)
            .min()
            .unwrap_or_else(|| current.saturating_add(1))
    }
}

struct RetiredHandle {
    handle: HandleId,
    epoch: u64,
}

/// 一个参与者在读取可移动对象地址前进入当前 epoch。
pub struct EpochParticipant {
    owner: Arc<EpochQuarantine>,
    id: u64,
    epoch: Arc<AtomicU64>,
}

impl EpochParticipant {
    pub fn enter(&self) {
        let epoch = self.owner.current.load(Ordering::SeqCst);
        self.epoch.store(epoch, Ordering::SeqCst);
    }

    pub fn exit(&self) {
        self.epoch.store(INACTIVE_EPOCH, Ordering::SeqCst);
    }
}

impl Drop for EpochParticipant {
    fn drop(&mut self) {
        self.exit();
        self.owner
            .participants
            .lock()
            .expect("epoch participant lock poisoned")
            .remove(&self.id);
    }
}
