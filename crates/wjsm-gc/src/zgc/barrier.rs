//! Colored load/store barrier protocol for Generational ZGC V2.
//!
//! Shared heap words use SeqCst atomics. NaN-box color bits (38–43) attach only
//! to handle-backed references; non-reference values keep those bits zero.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use parking_lot::Mutex;
use wjsm_ir::value::{
    self, GcColorMask, apply_gc_color, has_old_mark_color, has_remembered_color,
    has_young_mark_color, strip_gc_color,
};

use crate::heap::{ColoredHandleEntry, HandleGeneration, HandleId, HandleState, HandleTableV2};

/// load barrier 结果：稳定地址或需要 assist 的 relocating entry。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadBarrierOutcome {
    Stable {
        address: u64,
        generation: HandleGeneration,
    },
    Relocating {
        address: u64,
        generation: HandleGeneration,
    },
    Invalid,
}

/// store barrier 可能产生的 work 记录。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarrierRecord {
    Satb(HandleId),
    RememberedSlot { slot_addr: u64 },
}

/// 当前 young/old/remembered epoch 的 color 状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BarrierEpoch {
    pub young_mark: u8,
    pub old_mark: u8,
    pub remembered: u8,
    pub young_marking: bool,
    pub old_marking: bool,
}

impl BarrierEpoch {
    pub const IDLE: Self = Self {
        young_mark: 0b01,
        old_mark: 0b01,
        remembered: 0b01,
        young_marking: false,
        old_marking: false,
    };

    pub fn mask(self) -> GcColorMask {
        GcColorMask {
            young_mark: if self.young_marking {
                self.young_mark
            } else {
                0
            },
            old_mark: if self.old_marking { self.old_mark } else { 0 },
            remembered: self.remembered,
        }
    }

    pub fn flip_young(self) -> Self {
        Self {
            young_mark: flip_color(self.young_mark),
            remembered: flip_color(self.remembered),
            young_marking: true,
            ..self
        }
    }

    pub fn end_young(self) -> Self {
        Self {
            young_marking: false,
            ..self
        }
    }

    pub fn flip_old(self) -> Self {
        Self {
            old_mark: flip_color(self.old_mark),
            old_marking: true,
            ..self
        }
    }

    pub fn end_old(self) -> Self {
        Self {
            old_marking: false,
            ..self
        }
    }
}

const fn flip_color(bits: u8) -> u8 {
    match bits & 0b11 {
        0b01 => 0b10,
        _ => 0b01,
    }
}

/// per-mutator preallocated SATB / remset ring。
#[derive(Debug)]
pub struct BarrierRing<T> {
    slots: Mutex<Vec<Option<T>>>,
    capacity: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
    host_flushes: AtomicUsize,
}

impl<T: Copy> BarrierRing<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "barrier ring capacity must be nonzero");
        Self {
            slots: Mutex::new(vec![None; capacity]),
            capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            host_flushes: AtomicUsize::new(0),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn host_flushes(&self) -> usize {
        self.host_flushes.load(Ordering::SeqCst)
    }

    /// 有空间时绝不进入 host；满时返回 false 供 mutator assist/drain。
    pub fn try_push(&self, value: T) -> bool {
        let mut slots = self.slots.lock();
        let head = self.head.load(Ordering::SeqCst);
        let tail = self.tail.load(Ordering::SeqCst);
        if head.wrapping_sub(tail) >= self.capacity {
            return false;
        }
        let index = head % self.capacity;
        slots[index] = Some(value);
        self.head.store(head.wrapping_add(1), Ordering::SeqCst);
        true
    }

    pub fn push_or_mark_full(&self, value: T) -> Result<(), T> {
        if self.try_push(value) {
            Ok(())
        } else {
            self.host_flushes.fetch_add(1, Ordering::SeqCst);
            Err(value)
        }
    }

    pub fn drain(&self) -> Vec<T> {
        let mut slots = self.slots.lock();
        let mut out = Vec::new();
        loop {
            let tail = self.tail.load(Ordering::SeqCst);
            let head = self.head.load(Ordering::SeqCst);
            if tail == head {
                break;
            }
            let index = tail % self.capacity;
            let value = slots[index].take();
            self.tail.store(tail.wrapping_add(1), Ordering::SeqCst);
            if let Some(value) = value {
                out.push(value);
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.head
            .load(Ordering::SeqCst)
            .wrapping_sub(self.tail.load(Ordering::SeqCst))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// load barrier：SeqCst 读 handle entry，Stable* 直出，Relocating* 要求 assist。
pub fn load_barrier(handles: &HandleTableV2, handle: HandleId) -> LoadBarrierOutcome {
    match handles.resolve(handle) {
        Some(entry) => classify_entry(entry),
        None => LoadBarrierOutcome::Invalid,
    }
}

pub fn classify_entry(entry: ColoredHandleEntry) -> LoadBarrierOutcome {
    match entry.state() {
        HandleState::StableYoung | HandleState::StableOld | HandleState::PinnedOld => {
            LoadBarrierOutcome::Stable {
                address: entry.address(),
                generation: entry.generation(),
            }
        }
        HandleState::RelocatingYoung | HandleState::RelocatingOld => {
            LoadBarrierOutcome::Relocating {
                address: entry.address(),
                generation: entry.generation(),
            }
        }
        HandleState::Free | HandleState::Retired => LoadBarrierOutcome::Invalid,
    }
}

/// store barrier：读旧 word，决定 SATB / remset，并仅给引用着色。
pub fn store_barrier(
    epoch: BarrierEpoch,
    owner_generation: HandleGeneration,
    slot: &AtomicU64,
    new_value: i64,
    slot_addr: u64,
) -> (i64, Vec<BarrierRecord>) {
    let old_raw = slot.load(Ordering::SeqCst) as i64;
    let colored = color_stored_value(epoch, new_value);
    slot.store(colored as u64, Ordering::SeqCst);

    let mut records = Vec::new();
    if (epoch.young_marking || epoch.old_marking)
        && let Some(old_handle) = reference_handle(old_raw)
    {
        let needs_satb = match owner_generation {
            HandleGeneration::Young => {
                epoch.young_marking && !has_young_mark_color(old_raw, epoch.young_mark)
            }
            HandleGeneration::Old => {
                (epoch.old_marking && !has_old_mark_color(old_raw, epoch.old_mark))
                    || (epoch.young_marking && !has_young_mark_color(old_raw, epoch.young_mark))
            }
        };
        if needs_satb {
            records.push(BarrierRecord::Satb(old_handle));
        }
    }

    if owner_generation == HandleGeneration::Old {
        if let Some(new_handle) = reference_handle(colored) {
            // remembered set tracks old→young edges; generation of target is supplied by caller via color.
            if !has_remembered_color(colored, epoch.remembered)
                && is_young_reference_hint(colored, epoch)
            {
                records.push(BarrierRecord::RememberedSlot { slot_addr });
            }
            let _ = new_handle;
        } else if reference_handle(old_raw).is_some() {
            // overwrite/delete of old→young edge still dirties the slot for precision rebuild.
            records.push(BarrierRecord::RememberedSlot { slot_addr });
        }
    }

    (colored, records)
}

/// 仅当 new 值是 handle-backed reference 时附着当前 epoch color。
pub fn color_stored_value(epoch: BarrierEpoch, value: i64) -> i64 {
    if !value::is_handle_backed_reference(value) {
        let stripped = strip_gc_color(value);
        debug_assert_eq!(stripped as u64 & value::GC_COLOR_MASK, 0);
        return stripped;
    }
    apply_gc_color(strip_gc_color(value), epoch.mask())
}

pub fn reference_handle(value: i64) -> Option<HandleId> {
    value::is_handle_backed_reference(value).then(|| HandleId::new(value::decode_handle(value)))
}

fn is_young_reference_hint(value: i64, epoch: BarrierEpoch) -> bool {
    // Without resolve, young-mark color absence after store coloring is not decisive.
    // Callers that know generation should use `store_barrier_with_target_generation`.
    let _ = (value, epoch);
    true
}

/// 精确 old→young 判定版本：target generation 已知时使用。
pub fn store_barrier_with_target_generation(
    epoch: BarrierEpoch,
    owner_generation: HandleGeneration,
    target_generation: Option<HandleGeneration>,
    slot: &AtomicU64,
    new_value: i64,
    slot_addr: u64,
) -> (i64, Vec<BarrierRecord>) {
    let old_raw = slot.load(Ordering::SeqCst) as i64;
    let colored = color_stored_value(epoch, new_value);
    slot.store(colored as u64, Ordering::SeqCst);

    let mut records = Vec::new();
    if (epoch.young_marking || epoch.old_marking)
        && let Some(old_handle) = reference_handle(old_raw)
    {
        let needs_satb = match owner_generation {
            HandleGeneration::Young => {
                epoch.young_marking && !has_young_mark_color(old_raw, epoch.young_mark)
            }
            HandleGeneration::Old => {
                (epoch.old_marking && !has_old_mark_color(old_raw, epoch.old_mark))
                    || (epoch.young_marking && !has_young_mark_color(old_raw, epoch.young_mark))
            }
        };
        if needs_satb {
            records.push(BarrierRecord::Satb(old_handle));
        }
    }

    if owner_generation == HandleGeneration::Old {
        match target_generation {
            Some(HandleGeneration::Young) => {
                records.push(BarrierRecord::RememberedSlot { slot_addr });
            }
            Some(HandleGeneration::Old) | None => {
                if reference_handle(old_raw).is_some() && target_generation.is_none() {
                    records.push(BarrierRecord::RememberedSlot { slot_addr });
                }
            }
        }
    }

    (colored, records)
}

/// mutable-in-place header field classification for verifier / relocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeaderFieldKind {
    ImmutableByteCopy,
    MutableAtomicWord,
    ReferenceSlot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeaderField {
    pub offset: u64,
    pub kind: HeaderFieldKind,
}

/// 静态 HeaderLayout：publish 后 immutable 字段可 byte-copy；mutable 必须逐 word SeqCst。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeaderLayout {
    pub fields: &'static [HeaderField],
}

impl HeaderLayout {
    pub const OBJECT: Self = Self {
        fields: &[
            HeaderField {
                offset: 0,
                kind: HeaderFieldKind::MutableAtomicWord, // prototype + type bits
            },
            HeaderField {
                offset: 8,
                kind: HeaderFieldKind::ImmutableByteCopy, // capacity / size
            },
        ],
    };

    pub fn rejects_bulk_copy_of_mutable_headers(self) -> bool {
        self.fields.iter().any(|field| {
            matches!(
                field.kind,
                HeaderFieldKind::MutableAtomicWord | HeaderFieldKind::ReferenceSlot
            )
        })
    }
}

/// bulk copy verifier：按 source/destination generation 选择逐槽 barrier 或 publish copy。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BulkCopyMode {
    /// 对象尚未 publish：允许 raw copy。
    PrePublish,
    /// 同 generation 且已证明无 concurrent mutation 的 collector copy。
    SafePublishCopy,
    /// 必须逐槽 store barrier。
    PerSlotBarrier,
}

pub fn select_bulk_copy_mode(
    published: bool,
    source_generation: HandleGeneration,
    destination_generation: HandleGeneration,
    layout: HeaderLayout,
) -> BulkCopyMode {
    if !published {
        return BulkCopyMode::PrePublish;
    }
    if layout.rejects_bulk_copy_of_mutable_headers() {
        return BulkCopyMode::PerSlotBarrier;
    }
    if source_generation == destination_generation {
        BulkCopyMode::SafePublishCopy
    } else {
        BulkCopyMode::PerSlotBarrier
    }
}

/// verifier 拒绝把 prototype 归类为 publish 后 immutable。
pub fn prototype_field_kind() -> HeaderFieldKind {
    HeaderFieldKind::MutableAtomicWord
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::value::{encode_f64, encode_object_handle, encode_runtime_string_handle};

    #[test]
    fn non_reference_values_never_receive_color_bits() {
        let epoch = BarrierEpoch {
            young_marking: true,
            ..BarrierEpoch::IDLE
        };
        let slot = AtomicU64::new(0);
        let (stored, records) = store_barrier(
            epoch,
            HandleGeneration::Young,
            &slot,
            encode_f64(1.25),
            0x1000,
        );
        assert_eq!(stored as u64 & value::GC_COLOR_MASK, 0);
        assert!(records.is_empty());
    }

    #[test]
    fn runtime_string_reference_receives_color() {
        let epoch = BarrierEpoch {
            young_marking: true,
            young_mark: 0b01,
            ..BarrierEpoch::IDLE
        };
        let value = encode_runtime_string_handle(9);
        let colored = color_stored_value(epoch, value);
        assert!(has_young_mark_color(colored, 0b01));
        assert_eq!(strip_gc_color(colored), value);
    }

    #[test]
    fn one_slot_ring_requires_assist_when_full() {
        let ring = BarrierRing::with_capacity(1);
        assert!(ring.try_push(HandleId::new(1)));
        assert!(!ring.try_push(HandleId::new(2)));
        assert_eq!(ring.drain(), vec![HandleId::new(1)]);
        assert!(ring.try_push(HandleId::new(2)));
    }

    #[test]
    fn prototype_is_mutable_header_not_immutable() {
        assert_eq!(prototype_field_kind(), HeaderFieldKind::MutableAtomicWord);
        assert!(HeaderLayout::OBJECT.rejects_bulk_copy_of_mutable_headers());
        assert_eq!(
            select_bulk_copy_mode(
                true,
                HandleGeneration::Young,
                HandleGeneration::Young,
                HeaderLayout::OBJECT
            ),
            BulkCopyMode::PerSlotBarrier
        );
    }

    #[test]
    fn satb_records_overwritten_old_reference_during_young_mark() {
        let epoch = BarrierEpoch {
            young_marking: true,
            young_mark: 0b01,
            ..BarrierEpoch::IDLE
        };
        let old = encode_object_handle(7);
        let slot = AtomicU64::new(old as u64);
        let (_stored, records) = store_barrier_with_target_generation(
            epoch,
            HandleGeneration::Young,
            Some(HandleGeneration::Young),
            &slot,
            encode_object_handle(8),
            0x2000,
        );
        assert!(records.contains(&BarrierRecord::Satb(HandleId::new(7))));
    }

    #[test]
    fn old_to_young_store_records_remembered_slot() {
        let epoch = BarrierEpoch::IDLE;
        let slot = AtomicU64::new(0);
        let (_stored, records) = store_barrier_with_target_generation(
            epoch,
            HandleGeneration::Old,
            Some(HandleGeneration::Young),
            &slot,
            encode_object_handle(3),
            0x3000,
        );
        assert_eq!(
            records,
            vec![BarrierRecord::RememberedSlot { slot_addr: 0x3000 }]
        );
    }
}
