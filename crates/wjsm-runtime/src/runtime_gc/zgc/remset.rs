//! Precise old→young remembered set, age, and in-place promotion.

#![cfg(feature = "managed-heap-v2")]

use std::collections::{BTreeMap, BTreeSet};

use parking_lot::Mutex;

use crate::heap::{HandleId, HandleTableV2, HandleTableError};

/// precise old→young remembered set：slot bitmap + double buffer。
#[derive(Debug, Default)]
pub struct PreciseRemset {
    active: Mutex<BTreeSet<u64>>,
    snapshot: Mutex<BTreeSet<u64>>,
    /// page → slot offsets for dense bit packing / dedup.
    slot_bits: Mutex<BTreeMap<u64, BTreeSet<u16>>>,
}

impl PreciseRemset {
    pub fn record_slot(&self, slot_addr: u64) {
        let page = slot_addr & !0xFFFF;
        let offset = (slot_addr & 0xFFFF) as u16;
        self.slot_bits
            .lock()
            .entry(page)
            .or_default()
            .insert(offset);
        self.active.lock().insert(slot_addr);
    }

    pub fn snapshot_and_flip(&self) -> Vec<u64> {
        let mut active = self.active.lock();
        let mut snapshot = self.snapshot.lock();
        *snapshot = std::mem::take(&mut *active);
        snapshot.iter().copied().collect()
    }

    pub fn clear_snapshot(&self) {
        self.snapshot.lock().clear();
    }

    pub fn active_len(&self) -> usize {
        self.active.lock().len()
    }

    pub fn contains_slot(&self, slot_addr: u64) -> bool {
        self.active.lock().contains(&slot_addr) || self.snapshot.lock().contains(&slot_addr)
    }

    pub fn slot_count_for_page(&self, page: u64) -> usize {
        self.slot_bits
            .lock()
            .get(&page)
            .map(BTreeSet::len)
            .unwrap_or(0)
    }
}

/// 原地晋升发布：与 young mark 竞争时通过 handle table promote CAS。
pub fn publish_promotion(handles: &HandleTableV2, handle: HandleId) -> Result<(), HandleTableError> {
    handles.promote(handle)
}
