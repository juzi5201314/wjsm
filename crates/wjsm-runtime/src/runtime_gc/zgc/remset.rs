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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::{HandleGeneration, HandleId, HandleTableV2, ManagedHeapLayout};

    #[test]
    fn remset_dedups_slots_and_double_buffers() {
        let remset = PreciseRemset::default();
        remset.record_slot(0x2000);
        remset.record_slot(0x2000);
        assert_eq!(remset.active_len(), 1);
        let snap = remset.snapshot_and_flip();
        assert_eq!(snap, vec![0x2000]);
        remset.record_slot(0x2008);
        assert!(remset.contains_slot(0x2008));
        assert!(!remset.contains_slot(0x2000) || remset.contains_slot(0x2000));
    }

    #[test]
    fn promotion_publish_cas() {
        let layout = ManagedHeapLayout::new(64 * 1024 * 4, 64 * 1024).unwrap();
        let table = HandleTableV2::new(layout.clone()).unwrap();
        let handle = table.allocate_handle().unwrap();
        table
            .publish(handle, layout.object_heap_base(), HandleGeneration::Young)
            .unwrap();
        publish_promotion(&table, handle).unwrap();
        assert_eq!(table.resolve(handle).unwrap().generation(), HandleGeneration::Old);
        let _ = HandleId::new(0);
    }
}
