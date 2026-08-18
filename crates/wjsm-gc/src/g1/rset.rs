use std::collections::{BTreeMap, BTreeSet};

use wjsm_ir::value;

use super::region::RegionKind;

const PRECISE_SLOT_THRESHOLD: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotOwner {
    pub region_idx: usize,
    pub kind: RegionKind,
}

#[derive(Debug, Default)]
pub struct G1RSet {
    dirty_cards: BTreeSet<usize>,
    precise_slots: BTreeMap<usize, BTreeSet<usize>>,
    card_write_counts: BTreeMap<usize, u8>,
    satb_handles: Vec<u32>,
    barrier_events: usize,
    satb_flushes: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct G1RSetStats {
    pub satb_flushes: usize,
    pub barrier_events: usize,
    pub dirty_cards: usize,
    pub precise_slots: usize,
}

impl G1RSet {
    pub fn record_write(
        &mut self,
        slot_addr: usize,
        old_value: i64,
        _new_value: i64,
        owner: SlotOwner,
        card_idx: usize,
        new_owner: Option<SlotOwner>,
    ) {
        self.barrier_events = self.barrier_events.saturating_add(1);
        if let Some(old_handle) = value_to_handle(old_value) {
            self.satb_handles.push(old_handle);
        }
        if needs_rset_edge(owner.kind, new_owner.map(|owner| owner.kind)) {
            self.mark_dirty(slot_addr, card_idx);
        }
    }

    #[cfg(test)]
    pub fn dirty_cards(&self) -> impl Iterator<Item = usize> + '_ {
        self.dirty_cards.iter().copied()
    }

    #[cfg(test)]
    pub fn precise_slots(&self, card_idx: usize) -> Option<impl Iterator<Item = usize> + '_> {
        self.precise_slots
            .get(&card_idx)
            .map(|slots| slots.iter().copied())
    }

    pub fn dirty_card_snapshot(&self) -> Vec<usize> {
        self.dirty_cards.iter().copied().collect()
    }

    pub fn clear_card(&mut self, card_idx: usize) {
        self.dirty_cards.remove(&card_idx);
        self.precise_slots.remove(&card_idx);
        self.card_write_counts.remove(&card_idx);
    }

    pub fn mark_dirty_slot(&mut self, slot_addr: usize, card_idx: usize) {
        self.mark_dirty(slot_addr, card_idx);
    }
    pub fn stats_snapshot(&self) -> G1RSetStats {
        G1RSetStats {
            satb_flushes: self.satb_flushes,
            barrier_events: self.barrier_events,
            dirty_cards: self.dirty_cards.len(),
            precise_slots: self.precise_slots.values().map(BTreeSet::len).sum(),
        }
    }

    #[cfg(test)]
    pub fn satb_handles(&self) -> &[u32] {
        &self.satb_handles
    }

    fn mark_dirty(&mut self, slot_addr: usize, card_idx: usize) {
        self.dirty_cards.insert(card_idx);
        let count = self.card_write_counts.entry(card_idx).or_default();
        *count = count.saturating_add(1);
        if *count >= PRECISE_SLOT_THRESHOLD {
            self.precise_slots
                .entry(card_idx)
                .or_default()
                .insert(slot_addr);
        }
    }
}

pub fn value_to_handle(value: i64) -> Option<u32> {
    value::tag_needs_root(value).then(|| value::decode_handle(value))
}

#[cfg(test)]
pub fn slot_card_index(object_heap_start: usize, slot_addr: usize) -> Option<usize> {
    slot_addr
        .checked_sub(object_heap_start)
        .map(|offset| offset / super::region::CARD_SIZE)
}

fn needs_rset_edge(owner_kind: RegionKind, new_kind: Option<RegionKind>) -> bool {
    matches!(owner_kind, RegionKind::Old)
        && matches!(new_kind, Some(RegionKind::Eden | RegionKind::Survivor))
}

#[cfg(test)]
mod tests {
    use super::super::region::CARD_SIZE;
    use super::*;

    fn old_owner() -> SlotOwner {
        SlotOwner {
            region_idx: 1,
            kind: RegionKind::Old,
        }
    }

    fn young_owner() -> SlotOwner {
        SlotOwner {
            region_idx: 2,
            kind: RegionKind::Eden,
        }
    }

    #[test]
    fn slot_card_index_uses_object_heap_start() {
        let start = 64 * 1024;
        assert_eq!(slot_card_index(start, start), Some(0));
        assert_eq!(slot_card_index(start, start + CARD_SIZE), Some(1));
        assert_eq!(slot_card_index(start, start - 1), None);
    }

    #[test]
    fn sparse_dirty_cards_iterate_in_order() {
        let mut rset = G1RSet::default();
        rset.record_write(
            4096,
            0,
            value::encode_object_handle(7),
            old_owner(),
            8,
            Some(young_owner()),
        );
        rset.record_write(
            1024,
            0,
            value::encode_object_handle(8),
            old_owner(),
            2,
            Some(young_owner()),
        );

        assert_eq!(rset.dirty_cards().collect::<Vec<_>>(), vec![2, 8]);
    }

    #[test]
    fn hot_card_upgrades_to_precise_slot_bitmap() {
        let mut rset = G1RSet::default();
        for i in 0..PRECISE_SLOT_THRESHOLD {
            rset.record_write(
                8192 + i as usize * 8,
                0,
                value::encode_object_handle(i as u32),
                old_owner(),
                16,
                Some(young_owner()),
            );
        }

        let slots = rset
            .precise_slots(16)
            .expect("hot card should have precise slots")
            .collect::<Vec<_>>();
        assert_eq!(
            slots,
            vec![8192 + (PRECISE_SLOT_THRESHOLD - 1) as usize * 8]
        );
    }

    #[test]
    fn satb_records_old_nan_boxed_handle_value() {
        let mut rset = G1RSet::default();
        rset.record_write(
            4096,
            value::encode_object_handle(42),
            value::encode_object_handle(7),
            old_owner(),
            8,
            Some(young_owner()),
        );

        assert_eq!(rset.satb_handles(), &[42]);
    }

    #[test]
    fn non_old_to_young_write_only_records_satb() {
        let mut rset = G1RSet::default();
        rset.record_write(
            4096,
            value::encode_object_handle(11),
            value::encode_object_handle(12),
            young_owner(),
            8,
            Some(young_owner()),
        );

        assert_eq!(rset.satb_handles(), &[11]);
        assert_eq!(rset.dirty_cards().collect::<Vec<_>>(), Vec::<usize>::new());
    }
}
