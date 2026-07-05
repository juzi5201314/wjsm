use std::collections::{BTreeMap, BTreeSet};

use wjsm_ir::{constants, value};

use super::region::{CARD_SIZE, RegionKind};

pub const BARRIER_EVENT_SIZE: usize = constants::GC_BARRIER_EVENT_SIZE as usize;
const PRECISE_SLOT_THRESHOLD: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarrierEvent {
    pub flags: u32,
    pub slot_addr: u32,
    pub old_value: i64,
    pub new_value: i64,
}

#[allow(dead_code)]
impl BarrierEvent {
    pub fn encode(self, out: &mut [u8]) -> bool {
        if out.len() != BARRIER_EVENT_SIZE {
            return false;
        }
        out[0..4].copy_from_slice(&self.flags.to_le_bytes());
        out[4..8].copy_from_slice(&self.slot_addr.to_le_bytes());
        out[8..16].copy_from_slice(&self.old_value.to_le_bytes());
        out[16..24].copy_from_slice(&self.new_value.to_le_bytes());
        true
    }

    pub fn decode(input: &[u8]) -> Option<Self> {
        if input.len() != BARRIER_EVENT_SIZE {
            return None;
        }
        Some(Self {
            flags: u32::from_le_bytes(input[0..4].try_into().ok()?),
            slot_addr: u32::from_le_bytes(input[4..8].try_into().ok()?),
            old_value: i64::from_le_bytes(input[8..16].try_into().ok()?),
            new_value: i64::from_le_bytes(input[16..24].try_into().ok()?),
        })
    }
}

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
}

#[allow(dead_code)]
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
        if let Some(old_handle) = value_to_handle(old_value) {
            self.satb_handles.push(old_handle);
        }
        if needs_rset_edge(owner.kind, new_owner.map(|owner| owner.kind)) {
            self.mark_dirty(slot_addr, card_idx);
        }
    }

    pub fn record_event(
        &mut self,
        event: BarrierEvent,
        owner: SlotOwner,
        card_idx: usize,
        new_owner: Option<SlotOwner>,
    ) {
        self.record_write(
            event.slot_addr as usize,
            event.old_value,
            event.new_value,
            owner,
            card_idx,
            new_owner,
        );
    }

    pub fn dirty_cards(&self) -> impl Iterator<Item = usize> + '_ {
        self.dirty_cards.iter().copied()
    }

    pub fn precise_slots(&self, card_idx: usize) -> Option<impl Iterator<Item = usize> + '_> {
        self.precise_slots
            .get(&card_idx)
            .map(|slots| slots.iter().copied())
    }

    pub fn dirty_card_snapshot(&self) -> Vec<usize> {
        self.dirty_cards.iter().copied().collect()
    }

    pub fn precise_slot_snapshot(&self, card_idx: usize) -> Vec<usize> {
        self.precise_slots
            .get(&card_idx)
            .map(|slots| slots.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn clear_card(&mut self, card_idx: usize) {
        self.dirty_cards.remove(&card_idx);
        self.precise_slots.remove(&card_idx);
        self.card_write_counts.remove(&card_idx);
    }

    pub fn clear_card_range(&mut self, start: usize, end: usize) {
        for card_idx in start..end {
            self.clear_card(card_idx);
        }
    }

    pub fn mark_dirty_slot(&mut self, slot_addr: usize, card_idx: usize) {
        self.mark_dirty(slot_addr, card_idx);
    }
    pub fn satb_handles(&self) -> &[u32] {
        &self.satb_handles
    }

    pub fn drain_satb_handles(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.satb_handles)
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

#[allow(dead_code)]
pub fn slot_card_index(object_heap_start: usize, slot_addr: usize) -> Option<usize> {
    slot_addr
        .checked_sub(object_heap_start)
        .map(|offset| offset / CARD_SIZE)
}

pub fn decode_buffer(input: &[u8]) -> impl Iterator<Item = BarrierEvent> + '_ {
    input
        .chunks_exact(BARRIER_EVENT_SIZE)
        .filter_map(BarrierEvent::decode)
}

fn needs_rset_edge(owner_kind: RegionKind, new_kind: Option<RegionKind>) -> bool {
    matches!(
        owner_kind,
        RegionKind::Old
            | RegionKind::HumongousStart
            | RegionKind::HumongousCont
            | RegionKind::Immortal
    ) && matches!(new_kind, Some(RegionKind::Eden | RegionKind::Survivor))
}

#[cfg(test)]
mod tests {
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
    fn event_is_exactly_24_bytes_and_round_trips() {
        let event = BarrierEvent {
            flags: 0xA5A5_0001,
            slot_addr: 0x1000,
            old_value: value::encode_object_handle(3),
            new_value: value::encode_object_handle(4),
        };
        let mut bytes = [0u8; BARRIER_EVENT_SIZE];

        assert!(event.encode(&mut bytes));
        assert_eq!(bytes.len(), 24);
        assert_eq!(BarrierEvent::decode(&bytes), Some(event));
    }

    #[test]
    fn full_24k_buffer_decodes_last_old_to_young_edge() {
        let event_count = constants::GC_BARRIER_EVENT_BUFFER_SIZE as usize / BARRIER_EVENT_SIZE;
        let mut bytes = vec![0u8; constants::GC_BARRIER_EVENT_BUFFER_SIZE as usize];
        let last = BarrierEvent {
            flags: 0,
            slot_addr: 0xDEAD,
            old_value: value::encode_object_handle(99),
            new_value: value::encode_object_handle(100),
        };
        last.encode(&mut bytes[(event_count - 1) * BARRIER_EVENT_SIZE..]);

        let decoded = decode_buffer(&bytes).collect::<Vec<_>>();
        assert_eq!(decoded.len(), event_count);
        assert_eq!(decoded[event_count - 1], last);
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
