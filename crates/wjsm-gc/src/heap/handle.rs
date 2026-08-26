//! V2 的 8-byte atomic handle table（后端无关）。
//!
//! handle region 的物理后端经 [`HandleRegionBackend`] 抽象；native production
//! 使用平台虚拟内存，测试可注入协议验证后端。`HandleTableV2` 的算法逻辑
//! （entry/commit/epoch）与物理映射解耦。

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use super::epoch::{EpochParticipant, HeapEpoch};
use super::handle_entry::{
    ColoredHandleEntry, HANDLE_ENTRY_BYTES, HANDLE_REGION_BYTES, HEAP_COMMIT_GRANULE_BYTES,
    HandleGeneration, HandleId, HandleState, HandleTableError,
};
use super::layout::ManagedHeapLayout;

const HANDLE_BLOCK_ENTRIES: usize = (HEAP_COMMIT_GRANULE_BYTES / HANDLE_ENTRY_BYTES) as usize;
const HANDLE_BLOCKS: usize = HANDLE_REGION_BYTES as usize / HEAP_COMMIT_GRANULE_BYTES as usize;

fn reservation_error(error: impl std::fmt::Display) -> HandleTableError {
    HandleTableError::VirtualReservation {
        detail: error.to_string(),
    }
}

/// handle region 的物理内存后端。
///
/// 实现方负责保留 [`HANDLE_REGION_BYTES`] 的连续地址空间并保证其在
/// `base_ptr` 生命周期内稳定映射。GC 只经 `base_ptr` 以 `AtomicU64` 访问 entry。
/// 对应 granule 变为可读写；已全量可写的测试后端可 no-op。
pub trait HandleRegionBackend: Send + Sync {
    /// region 基址（已按 8-byte 对齐，覆盖整个 HANDLE_REGION_BYTES 保留区）。
    fn base_ptr(&self) -> *mut u8;

    /// 提交 `[offset, offset+len)` 为可读写。默认 no-op（已可写后端）。
    fn commit_block(&self, _offset: usize, _len: usize) -> Result<(), HandleTableError> {
        Ok(())
    }
}

/// 基于平台虚拟内存的 handle region 后端（后端无关默认实现）。
///
/// 用 `platform::reserve` 保留 [`HANDLE_REGION_BYTES`] 匿名虚拟内存；
/// `VirtualRange` 为 RAII，保活映射直到 region drop。
pub struct PlatformHandleRegion {
    range: parking_lot::Mutex<super::platform::VirtualRange>,
}

// SAFETY: `VirtualRange.base` 指向独立保留的虚拟内存区，仅经原子操作访问。
unsafe impl Send for PlatformHandleRegion {}
// SAFETY: 同上；region 不持可变借用，所有访问经 `AtomicU64::from_ptr`。
unsafe impl Sync for PlatformHandleRegion {}

impl PlatformHandleRegion {
    /// 保留一块 [`HANDLE_REGION_BYTES`] 的平台虚拟内存作为 handle region。
    pub fn reserve() -> Result<Self, HandleTableError> {
        let range =
            super::platform::reserve(HANDLE_REGION_BYTES as usize).map_err(reservation_error)?;
        if range.base().is_null() {
            return Err(HandleTableError::VirtualReservation {
                detail: "platform reserve returned null base".to_owned(),
            });
        }
        Ok(Self {
            range: parking_lot::Mutex::new(range),
        })
    }
}

impl HandleRegionBackend for PlatformHandleRegion {
    fn base_ptr(&self) -> *mut u8 {
        self.range.lock().base()
    }

    fn commit_block(&self, offset: usize, len: usize) -> Result<(), HandleTableError> {
        self.range
            .lock()
            .commit(offset, len)
            .map_err(reservation_error)
    }
}

/// V2 的连续 memory64 handle region；仅第一次发布 block 时增加 committed 计数。
struct HandleRegion {
    /// 保活物理映射（VirtualRange RAII 或测试后端资源）。
    backend: Box<dyn HandleRegionBackend>,
    base: usize,
    committed_blocks: Box<[AtomicU8]>,
    committed_bytes: AtomicU64,
}

impl HandleRegion {
    fn new(backend: Box<dyn HandleRegionBackend>) -> Result<Self, HandleTableError> {
        let base = backend.base_ptr() as usize;
        if base == 0 {
            return Err(HandleTableError::VirtualReservation {
                detail: "handle region backend returned a null base".to_owned(),
            });
        }
        let committed_blocks = std::iter::repeat_with(|| AtomicU8::new(0))
            .take(HANDLE_BLOCKS)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Ok(Self {
            backend,
            base,
            committed_blocks,
            committed_bytes: AtomicU64::new(0),
        })
    }

    #[inline(always)]
    fn entry(&self, handle: HandleId) -> &AtomicU64 {
        let offset = HandleTableV2::entry_address(handle) as usize;
        // SAFETY: backend 保证 base..base+HANDLE_REGION_BYTES 稳定映射；
        // handle offset 落在该范围且按 8-byte entry 对齐。该 region 只以 AtomicU64 访问。
        // 调用方须先 `commit` 使对应 block 可读写。
        unsafe { AtomicU64::from_ptr((self.base + offset) as *mut u64) }
    }

    fn is_committed(&self, handle: HandleId) -> bool {
        let block = handle.get() as usize / HANDLE_BLOCK_ENTRIES;
        self.committed_blocks[block].load(Ordering::Acquire) == 2
    }

    fn load_entry(&self, handle: HandleId) -> u64 {
        if !self.is_committed(handle) {
            return 0;
        }
        self.entry(handle).load(Ordering::SeqCst)
    }

    fn commit(&self, handle: HandleId) -> Result<(), HandleTableError> {
        self.commit_block(handle.get() as usize / HANDLE_BLOCK_ENTRIES)
    }

    fn commit_range(&self, start: u32, limit: u32) -> Result<(), HandleTableError> {
        if start >= limit {
            return Err(HandleTableError::InvalidHandleRange);
        }
        let first = start as usize / HANDLE_BLOCK_ENTRIES;
        let last = (limit - 1) as usize / HANDLE_BLOCK_ENTRIES;
        for block in first..=last {
            self.commit_block(block)?;
        }
        Ok(())
    }

    fn commit_block(&self, block: usize) -> Result<(), HandleTableError> {
        loop {
            match self.committed_blocks[block].load(Ordering::Acquire) {
                2 => return Ok(()),
                0 => {
                    if self.committed_blocks[block]
                        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                        .is_err()
                    {
                        continue;
                    }
                    let offset = block * HANDLE_BLOCK_ENTRIES * HANDLE_ENTRY_BYTES as usize;
                    let len = HEAP_COMMIT_GRANULE_BYTES as usize;
                    match self.backend.commit_block(offset, len) {
                        Ok(()) => {
                            self.committed_bytes
                                .fetch_add(HEAP_COMMIT_GRANULE_BYTES, Ordering::Relaxed);
                            self.committed_blocks[block].store(2, Ordering::Release);
                            return Ok(());
                        }
                        Err(error) => {
                            self.committed_blocks[block].store(0, Ordering::Release);
                            return Err(error);
                        }
                    }
                }
                1 => std::hint::spin_loop(),
                _ => unreachable!("handle commit state is 0, 1, or 2"),
            }
        }
    }

    fn committed_bytes(&self) -> u64 {
        self.committed_bytes.load(Ordering::SeqCst)
    }

    /// handle region 基址；generated code 用它把句柄下标换算成 entry 地址。
    const fn base_ptr(&self) -> *mut u8 {
        self.base as *mut u8
    }
}

/// V2 的 8-byte atomic handle table；不触碰 active 4-byte obj_table ABI。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RestoredHandleEntry {
    pub handle: HandleId,
    pub address: u64,
    pub generation: HandleGeneration,
}

/// 只包含 fresh monotonic handle 的连续 reservation；epoch reusable slot 不会进入其中。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandleRangeReservation {
    start: u32,
    limit: u32,
}

impl HandleRangeReservation {
    pub const fn start(&self) -> u32 {
        self.start
    }

    pub const fn limit(&self) -> u32 {
        self.limit
    }

    pub const fn len(&self) -> u32 {
        self.limit - self.start
    }

    pub const fn contains(&self, handle: u32) -> bool {
        handle >= self.start && handle < self.limit
    }
}

pub struct HandleTableV2 {
    layout: ManagedHeapLayout,
    region: HandleRegion,
    next_handle: AtomicU64,
    epochs: Arc<HeapEpoch>,
}

impl HandleTableV2 {
    /// 用平台虚拟内存后端创建 handle table（后端无关默认）。
    pub fn new(layout: ManagedHeapLayout) -> Result<Self, HandleTableError> {
        Self::with_epoch(layout, HeapEpoch::new())
    }

    pub fn with_epoch(
        layout: ManagedHeapLayout,
        epochs: Arc<HeapEpoch>,
    ) -> Result<Self, HandleTableError> {
        Self::with_backend_and_epoch(layout, Box::new(PlatformHandleRegion::reserve()?), epochs)
    }

    /// 用指定物理后端创建 handle table；production 传入平台 region，测试传入协议后端。
    pub fn with_backend(
        layout: ManagedHeapLayout,
        backend: Box<dyn HandleRegionBackend>,
    ) -> Result<Self, HandleTableError> {
        Self::with_backend_and_epoch(layout, backend, HeapEpoch::new())
    }

    pub fn with_backend_and_epoch(
        layout: ManagedHeapLayout,
        backend: Box<dyn HandleRegionBackend>,
        epochs: Arc<HeapEpoch>,
    ) -> Result<Self, HandleTableError> {
        Ok(Self {
            layout,
            region: HandleRegion::new(backend)?,
            next_handle: AtomicU64::new(0),
            epochs,
        })
    }

    pub const fn layout(&self) -> &ManagedHeapLayout {
        &self.layout
    }

    pub fn epoch(&self) -> Arc<HeapEpoch> {
        Arc::clone(&self.epochs)
    }

    pub const fn reserved_bytes(&self) -> u64 {
        HANDLE_REGION_BYTES
    }

    pub fn committed_bytes(&self) -> u64 {
        self.region.committed_bytes()
    }

    pub const fn block_bytes(&self) -> u64 {
        HEAP_COMMIT_GRANULE_BYTES
    }

    pub const fn entry_address(handle: HandleId) -> u64 {
        handle.get() as u64 * HANDLE_ENTRY_BYTES
    }

    /// handle region 基址（8 字节对齐，覆盖整个保留区）。generated code 通过
    /// vmctx 的 `handle_table_base` 读取；region 生命周期与 handle table 相同。
    pub const fn region_base(&self) -> *mut u8 {
        self.region.base_ptr()
    }

    pub fn allocate_handle(&self) -> Result<HandleId, HandleTableError> {
        if let Some(handle) = self.epochs.take_reusable() {
            let next = u64::from(handle.get()) + 1;
            self.next_handle.fetch_max(next, Ordering::SeqCst);
            return Ok(handle);
        }
        let raw = self
            .next_handle
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |next| {
                (next <= u64::from(u32::MAX)).then_some(next + 1)
            })
            .map_err(|_| HandleTableError::HandleExhausted)?;
        Ok(HandleId::new(raw as u32))
    }

    /// 一次性预留 fresh monotonic handle 区间，并预提交覆盖的 64 KiB blocks。
    ///
    /// reusable handle 只允许由 `allocate_handle` 的宿主慢路径消费，避免生成代码
    /// reservation 与 epoch reclaim 共享可复用 cursor。
    pub fn reserve_range(&self, count: u32) -> Result<HandleRangeReservation, HandleTableError> {
        if count == 0 {
            return Err(HandleTableError::InvalidHandleRange);
        }
        let mut start = self.next_handle.load(Ordering::Acquire);
        loop {
            let end = start
                .checked_add(u64::from(count))
                .filter(|end| *end <= u64::from(u32::MAX))
                .ok_or(HandleTableError::HandleExhausted)?;
            let start_raw = start as u32;
            let limit = end as u32;
            self.region.commit_range(start_raw, limit)?;
            let mut occupied_end = start;
            for raw in start_raw..limit {
                let state =
                    ColoredHandleEntry::from_raw(self.region.load_entry(HandleId::new(raw)))
                        .state();
                if !matches!(state, HandleState::Free | HandleState::Retired) {
                    occupied_end = u64::from(raw) + 1;
                }
            }
            if occupied_end != start {
                self.next_handle.fetch_max(occupied_end, Ordering::AcqRel);
                start = occupied_end;
                continue;
            }
            match self
                .next_handle
                .compare_exchange(start, end, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    return Ok(HandleRangeReservation {
                        start: start_raw,
                        limit,
                    });
                }
                Err(actual) => start = actual,
            }
        }
    }

    /// 发布已预留区间中的 stable entry。fast generated code 使用同一编码直接 release-store；
    /// 此方法供宿主 materialize、测试与 slow owner 共享验证逻辑。
    pub fn publish_reserved(
        &self,
        reservation: &HandleRangeReservation,
        handle: u32,
        address: u64,
        generation: HandleGeneration,
    ) -> Result<(), HandleTableError> {
        if !reservation.contains(handle) {
            return Err(HandleTableError::UnallocatedHandle {
                handle: HandleId::new(handle),
            });
        }
        if u64::from(handle) >= self.allocated_count() {
            return Err(HandleTableError::UnallocatedHandle {
                handle: HandleId::new(handle),
            });
        }
        self.require_object_address(address)?;
        let entry = ColoredHandleEntry::new(address, HandleState::stable_for(generation))?;
        let slot = self.region.entry(HandleId::new(handle));
        let current = ColoredHandleEntry::from_raw(slot.load(Ordering::Acquire));
        if current.state() != HandleState::Free {
            return Err(HandleTableError::InvalidTransition {
                handle: HandleId::new(handle),
                expected: HandleState::Free,
                actual: current.state(),
            });
        }
        slot.store(entry.raw(), Ordering::Release);
        Ok(())
    }

    pub fn allocated_count(&self) -> u64 {
        self.next_handle.load(Ordering::Acquire)
    }

    /// 捕获当前稳定 handle entry；free/retired slot 由 `next_handle` 保留为空洞。
    pub fn snapshot_entries(&self) -> Result<Vec<RestoredHandleEntry>, HandleTableError> {
        let next_handle = self.allocated_count();
        let mut entries = Vec::new();
        for raw in 0..next_handle {
            let handle = HandleId::new(raw as u32);
            let entry = ColoredHandleEntry::from_raw(self.region.load_entry(handle));
            let state = entry.state();
            if state.is_stable() {
                entries.push(RestoredHandleEntry {
                    handle,
                    address: entry.address(),
                    generation: entry.generation(),
                });
            } else if state != HandleState::Free && state != HandleState::Retired {
                return Err(HandleTableError::InvalidTransition {
                    handle,
                    expected: state
                        .generation()
                        .map_or(HandleState::StableYoung, HandleState::stable_for),
                    actual: state,
                });
            }
        }
        Ok(entries)
    }

    pub fn publish(
        &self,
        handle: HandleId,
        address: u64,
        generation: HandleGeneration,
    ) -> Result<(), HandleTableError> {
        self.require_object_address(address)?;
        if u64::from(handle.get()) >= self.allocated_count() {
            return Err(HandleTableError::UnallocatedHandle { handle });
        }
        self.region.commit(handle)?;
        let next = ColoredHandleEntry::new(address, HandleState::stable_for(generation))?;
        self.compare_exchange(handle, HandleState::Free, next)
    }
    pub fn restore_snapshot(
        &self,
        entries: &[RestoredHandleEntry],
        next_handle: u64,
    ) -> Result<(), HandleTableError> {
        if self.allocated_count() != 0 {
            return Err(HandleTableError::RestoreRequiresEmpty);
        }
        if next_handle > u64::from(u32::MAX) + 1 {
            return Err(HandleTableError::HandleExhausted);
        }
        let mut seen = HashSet::with_capacity(entries.len());
        for entry in entries {
            if u64::from(entry.handle.get()) >= next_handle {
                return Err(HandleTableError::RestoreHandleOutOfRange {
                    handle: entry.handle,
                    next_handle,
                });
            }
            if !seen.insert(entry.handle.get()) {
                return Err(HandleTableError::DuplicateRestoreHandle {
                    handle: entry.handle,
                });
            }
            self.require_object_address(entry.address)?;
        }
        for entry in entries {
            self.region.commit(entry.handle)?;
        }
        for entry in entries {
            let value =
                ColoredHandleEntry::new(entry.address, HandleState::stable_for(entry.generation))?;
            self.compare_exchange(entry.handle, HandleState::Free, value)?;
        }
        self.next_handle.store(next_handle, Ordering::Release);
        Ok(())
    }

    #[inline(always)]
    pub fn resolve(&self, handle: HandleId) -> Option<ColoredHandleEntry> {
        let entry = ColoredHandleEntry::from_raw(self.region.load_entry(handle));
        (!matches!(entry.state(), HandleState::Free | HandleState::Retired)).then_some(entry)
    }

    pub fn promote(&self, handle: HandleId) -> Result<(), HandleTableError> {
        let current = ColoredHandleEntry::from_raw(self.region.load_entry(handle));
        let next = ColoredHandleEntry::new(current.address(), HandleState::StableOld)?;
        self.compare_exchange(handle, HandleState::StableYoung, next)
    }

    pub fn begin_relocation(&self, handle: HandleId) -> Result<(), HandleTableError> {
        let current = ColoredHandleEntry::from_raw(self.region.load_entry(handle));
        let state = current.state();
        let generation = state
            .generation()
            .ok_or(HandleTableError::InvalidTransition {
                handle,
                expected: HandleState::StableYoung,
                actual: state,
            })?;
        if !state.is_stable() {
            return Err(HandleTableError::InvalidTransition {
                handle,
                expected: HandleState::StableYoung,
                actual: state,
            });
        }
        let next =
            ColoredHandleEntry::new(current.address(), HandleState::relocating_for(generation))?;
        self.compare_exchange(handle, state, next)
    }

    pub fn complete_relocation(
        &self,
        handle: HandleId,
        address: u64,
    ) -> Result<(), HandleTableError> {
        self.require_object_address(address)?;
        let current = ColoredHandleEntry::from_raw(self.region.load_entry(handle));
        let state = current.state();
        let generation = state
            .generation()
            .ok_or(HandleTableError::InvalidTransition {
                handle,
                expected: HandleState::RelocatingYoung,
                actual: state,
            })?;
        let expected = HandleState::relocating_for(generation);
        let next = ColoredHandleEntry::new(address, HandleState::stable_for(generation))?;
        self.compare_exchange(handle, expected, next)
    }

    pub fn retire(&self, handle: HandleId) -> Result<(), HandleTableError> {
        let current = ColoredHandleEntry::from_raw(self.region.load_entry(handle));
        let state = current.state();
        if !state.is_stable() {
            return Err(HandleTableError::InvalidTransition {
                handle,
                expected: HandleState::StableYoung,
                actual: state,
            });
        }
        let retired = ColoredHandleEntry::new(current.address(), HandleState::Retired)?;
        self.compare_exchange(handle, state, retired)?;
        self.epochs.retire_handle(handle);
        Ok(())
    }

    pub fn register_participant(&self) -> EpochParticipant {
        self.epochs.register()
    }

    pub fn advance_epoch(&self) -> u64 {
        self.epochs.advance()
    }

    pub fn reclaim_quarantine(&self) -> usize {
        let handles = self.epochs.take_reclaimable_handles();
        for handle in &handles {
            self.free_retired(*handle)
                .expect("retired handle state changed before epoch reclaim");
            self.epochs.make_reusable(*handle);
        }
        handles.len()
    }

    fn compare_exchange(
        &self,
        handle: HandleId,
        expected: HandleState,
        next: ColoredHandleEntry,
    ) -> Result<(), HandleTableError> {
        let slot = self.region.entry(handle);
        let current = ColoredHandleEntry::from_raw(slot.load(Ordering::SeqCst));
        if current.state() != expected {
            return Err(HandleTableError::InvalidTransition {
                handle,
                expected,
                actual: current.state(),
            });
        }
        slot.compare_exchange(
            current.raw(),
            next.raw(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .map(|_| ())
        .map_err(|actual| HandleTableError::InvalidTransition {
            handle,
            expected,
            actual: ColoredHandleEntry::from_raw(actual).state(),
        })
    }

    fn free_retired(&self, handle: HandleId) -> Result<(), HandleTableError> {
        let free = ColoredHandleEntry::new(0, HandleState::Free)?;
        self.compare_exchange(handle, HandleState::Retired, free)
    }

    fn require_object_address(&self, address: u64) -> Result<(), HandleTableError> {
        self.layout
            .contains_object_address(address)
            .then_some(())
            .ok_or(HandleTableError::AddressOutsideObjectHeap { address })
    }
}
