//! Colored load/store barrier protocol for Generational ZGC V2.
//!
//! Shared heap words use SeqCst atomics. NaN-box color bits (38–43) attach only
//! to handle-backed references; non-reference values keep those bits zero.

use parking_lot::RwLock;
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use wjsm_ir::value::{
    self, GcColorMask, apply_gc_color, has_old_mark_color, has_remembered_color,
    has_young_mark_color, strip_gc_color,
};

use crate::heap::{
    ColoredHandleEntry, GrowableHeapMemory, HandleGeneration, HandleId, HandleState, HandleTableV2,
};
use crate::zgc::ConcurrentRelocator;

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
    Satb(i64),
    Mark(i64),
    RememberedSlot { slot_addr: u64 },
    RememberedObject(HandleId),
}

type BarrierAssist = dyn Fn(BarrierRecord, u64) -> bool + Send + Sync;
const MUTATOR_ASSIST_LIMIT_BYTES: u64 = 256 * 1024;

/// 当前 young/old/remembered epoch 的 color 状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BarrierEpoch {
    pub young_mark: u8,
    pub old_mark: u8,
    pub remembered: u8,
    pub young_marking: bool,
    pub old_marking: bool,
}

/// 生产 heap access 的屏障模式；非 ZGC collector 不承担 ring/assist 成本。
pub enum HeapBarrier<M: GrowableHeapMemory> {
    Disabled,
    Zgc(Arc<ZgcBarrierSet<M>>),
}

/// ZGC 屏障共享状态；mutator 与 collector 只共享预分配记录区和原子 epoch。
pub struct ZgcBarrierSet<M: GrowableHeapMemory> {
    pub(crate) handles: Arc<HandleTableV2>,
    pub(crate) memory: M,
    pub(crate) packed_epoch: AtomicU64,
    pub(crate) access_epoch: AtomicU64,
    pub(crate) records: BarrierRing<BarrierRecord>,
    pub(crate) relocator: ConcurrentRelocator,
    assist: RwLock<Option<Arc<BarrierAssist>>>,
}

impl<M: GrowableHeapMemory> ZgcBarrierSet<M> {
    pub fn new(handles: Arc<HandleTableV2>, memory: M, ring_capacity: usize) -> Self {
        Self {
            handles,
            memory,
            packed_epoch: AtomicU64::new(BarrierEpoch::IDLE.pack()),
            access_epoch: AtomicU64::new(0),
            records: BarrierRing::with_capacity(ring_capacity),
            relocator: ConcurrentRelocator::new(),
            assist: RwLock::new(None),
        }
    }

    pub fn handles(&self) -> &HandleTableV2 {
        &self.handles
    }

    pub fn memory(&self) -> &M {
        &self.memory
    }

    pub fn epoch(&self) -> BarrierEpoch {
        BarrierEpoch::unpack(self.packed_epoch.load(Ordering::SeqCst))
    }

    pub fn access_epoch(&self) -> u64 {
        self.access_epoch.load(Ordering::SeqCst)
    }

    pub fn records(&self) -> &BarrierRing<BarrierRecord> {
        &self.records
    }

    pub fn relocator(&self) -> &ConcurrentRelocator {
        &self.relocator
    }

    pub(crate) fn install_assist(&self, assist: Arc<BarrierAssist>) {
        *self.assist.write() = Some(assist);
    }

    pub fn record(&self, record: BarrierRecord) -> Result<(), BarrierRecord> {
        match self.records.try_push(record) {
            Ok(()) => Ok(()),
            Err(record) => {
                if self
                    .assist
                    .read()
                    .as_ref()
                    .is_some_and(|assist| assist(record, MUTATOR_ASSIST_LIMIT_BYTES))
                {
                    Ok(())
                } else {
                    Err(record)
                }
            }
        }
    }

    pub fn set_epoch(&self, epoch: BarrierEpoch) {
        self.packed_epoch.store(epoch.pack(), Ordering::SeqCst);
    }

    pub fn publish_access_epoch(&self) {
        self.access_epoch
            .store(self.relocator.access_epoch(), Ordering::SeqCst);
    }

    pub fn drain_records(&self, visitor: impl FnMut(BarrierRecord)) {
        self.records.drain_into(visitor);
    }
}

impl BarrierEpoch {
    pub const IDLE: Self = Self {
        young_mark: 0b01,
        old_mark: 0b01,
        remembered: 0b01,
        young_marking: false,
        old_marking: false,
    };

    pub const fn pack(self) -> u64 {
        self.young_mark as u64
            | ((self.old_mark as u64) << 2)
            | ((self.remembered as u64) << 4)
            | ((self.young_marking as u64) << 6)
            | ((self.old_marking as u64) << 7)
    }
    pub const fn unpack(bits: u64) -> Self {
        Self {
            young_mark: (bits & 0b11) as u8,
            old_mark: ((bits >> 2) & 0b11) as u8,
            remembered: ((bits >> 4) & 0b11) as u8,
            young_marking: bits & (1 << 6) != 0,
            old_marking: bits & (1 << 7) != 0,
        }
    }

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

/// 预分配 SPSC 屏障 ring：单 mutator producer、单 collector consumer。
pub struct BarrierRing<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    capacity: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
    flush_debt: AtomicUsize,
}

// SAFETY: SPSC 契约保证每个 slot 同时至多被一个 producer 写或一个 consumer 读；
// head 的 Release/Acquire 发布写入，tail 的 Release/Acquire 发布消费完成。
unsafe impl<T: Copy + Send> Sync for BarrierRing<T> {}

impl<T: Copy> BarrierRing<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "barrier ring capacity must be nonzero");
        let slots = std::iter::repeat_with(|| UnsafeCell::new(MaybeUninit::uninit()))
            .take(capacity)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots,
            capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            flush_debt: AtomicUsize::new(0),
        }
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// 有空间时只写预分配 slot；满时发布 flush/assist debt 并返还原值。
    pub fn try_push(&self, value: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.capacity {
            self.flush_debt.fetch_add(1, Ordering::Release);
            return Err(value);
        }
        let index = head % self.capacity;
        // SAFETY: SPSC 契约保证 producer 独占 head slot；容量检查证明 consumer
        // 已释放该 slot，且 `index` 总在预分配数组范围内。
        unsafe {
            (*self.slots[index].get()).write(value);
        }
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    pub fn drain_into(&self, mut visitor: impl FnMut(T)) {
        let mut tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        while tail != head {
            let index = tail % self.capacity;
            // SAFETY: Acquire 读取 head 后，当前 tail slot 已由 producer 完整初始化；
            // 单 consumer 独占读取，并在 Release 更新 tail 前不会被 producer 复用。
            let value = unsafe { (*self.slots[index].get()).assume_init_read() };
            visitor(value);
            tail = tail.wrapping_add(1);
        }
        self.tail.store(tail, Ordering::Release);
    }

    pub fn take_flush_debt(&self) -> usize {
        self.flush_debt.swap(0, Ordering::AcqRel)
    }

    pub fn len(&self) -> usize {
        self.head
            .load(Ordering::Acquire)
            .wrapping_sub(self.tail.load(Ordering::Acquire))
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
    if (epoch.young_marking || epoch.old_marking) && reference_handle(old_raw).is_some() {
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
            records.push(BarrierRecord::Satb(value::strip_gc_color(old_raw)));
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
        return value;
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
    if (epoch.young_marking || epoch.old_marking) && reference_handle(old_raw).is_some() {
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
            records.push(BarrierRecord::Satb(value::strip_gc_color(old_raw)));
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

    /// 字符串对象：publish 后 `+0` proto（可换原型）与 `+24` hash（惰性内容哈希）
    /// 仍可能被 mutator 写；`+5 repr / +6 flags / +8 length / +12 capacity` 在
    /// publish 时一次写入后不可变。hash 一次写入后不再变，但写入时机在 GC 之后，
    /// 必须以 MutableAtomicWord 参与搬迁同步。
    pub const STRING: Self = Self {
        fields: &[
            HeaderField {
                offset: 0,
                kind: HeaderFieldKind::MutableAtomicWord, // prototype + type bits
            },
            HeaderField {
                offset: 24,
                kind: HeaderFieldKind::MutableAtomicWord, // hash（惰性写）
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
        assert_eq!(ring.try_push(HandleId::new(1)), Ok(()));
        assert_eq!(ring.try_push(HandleId::new(2)), Err(HandleId::new(2)));
        assert_eq!(ring.take_flush_debt(), 1);
        let mut drained = Vec::new();
        ring.drain_into(|handle| drained.push(handle));
        assert_eq!(drained, vec![HandleId::new(1)]);
        assert_eq!(ring.try_push(HandleId::new(2)), Ok(()));
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
        assert!(records.contains(&BarrierRecord::Satb(old)));
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
