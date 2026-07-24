//! V2 的 8-byte atomic handle table（后端无关）。
//!
//! handle region 的物理后端经 [`HandleRegionBackend`] 抽象：
//! wasm 后端用 wasmtime shared memory64（host-wasm 实现），native/独立 GC
//! 用平台虚拟内存（[`PlatformHandleRegion`]，本模块提供）。`HandleTableV2`
//! 的算法逻辑（entry/commit/epoch）与后端解耦。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::epoch::{EpochParticipant, EpochQuarantine};
use super::handle_entry::{
    ColoredHandleEntry, HANDLE_ENTRY_BYTES, HANDLE_REGION_BYTES, HEAP_COMMIT_GRANULE_BYTES,
    HandleGeneration, HandleId, HandleState, HandleTableError,
};
use super::layout::ManagedHeapLayout;

const HANDLE_BLOCK_ENTRIES: usize = (HEAP_COMMIT_GRANULE_BYTES / HANDLE_ENTRY_BYTES) as usize;
const COMMIT_BITMAP_WORDS: usize =
    (HANDLE_REGION_BYTES as usize / HANDLE_ENTRY_BYTES as usize) / (HANDLE_BLOCK_ENTRIES * 64);

fn reservation_error(error: impl std::fmt::Display) -> HandleTableError {
    HandleTableError::VirtualReservation {
        detail: error.to_string(),
    }
}

/// handle region 的物理内存后端。
///
/// 实现方负责保留 [`HANDLE_REGION_BYTES`] 的连续地址空间并保证其在
/// `base_ptr` 生命周期内稳定映射。GC 只经 `base_ptr` 以 `AtomicU64` 访问 entry。
/// 若后端以 `PROT_NONE` 预留（平台虚拟内存），必须在 `commit_block` 中把
/// 对应 granule 变为可读写；wasm shared memory 等已全量可写的后端可 no-op。
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
        let range = super::platform::reserve(HANDLE_REGION_BYTES as usize)
            .map_err(reservation_error)?;
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
    /// 保活物理映射（VirtualRange RAII / wasmtime SharedMemory）。
    backend: Box<dyn HandleRegionBackend>,
    base: usize,
    committed_blocks: Box<[AtomicU64]>,
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
        let committed_blocks = std::iter::repeat_with(|| AtomicU64::new(0))
            .take(COMMIT_BITMAP_WORDS)
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
        let word = block / u64::BITS as usize;
        let bit = 1_u64 << (block % u64::BITS as usize);
        self.committed_blocks[word].load(Ordering::SeqCst) & bit != 0
    }

    fn load_entry(&self, handle: HandleId) -> u64 {
        if !self.is_committed(handle) {
            return 0;
        }
        self.entry(handle).load(Ordering::SeqCst)
    }

    fn commit(&self, handle: HandleId) {
        let block = handle.get() as usize / HANDLE_BLOCK_ENTRIES;
        let word = block / u64::BITS as usize;
        let bit = 1_u64 << (block % u64::BITS as usize);
        let previous = self.committed_blocks[word].fetch_or(bit, Ordering::SeqCst);
        if previous & bit == 0 {
            let offset = block * HANDLE_BLOCK_ENTRIES * HANDLE_ENTRY_BYTES as usize;
            let len = HEAP_COMMIT_GRANULE_BYTES as usize;
            // 平台 PROT_NONE 预留必须 mprotect；失败时仍记 committed 并让后续访问暴露错误。
            let _ = self.backend.commit_block(offset, len);
            self.committed_bytes
                .fetch_add(HEAP_COMMIT_GRANULE_BYTES, Ordering::SeqCst);
        }
    }

    fn committed_bytes(&self) -> u64 {
        self.committed_bytes.load(Ordering::SeqCst)
    }
}

/// V2 的 8-byte atomic handle table；不触碰 active 4-byte obj_table ABI。
pub struct HandleTableV2 {
    layout: ManagedHeapLayout,
    region: HandleRegion,
    next_handle: AtomicU64,
    epochs: Arc<EpochQuarantine>,
}

impl HandleTableV2 {
    /// 用平台虚拟内存后端创建 handle table（后端无关默认）。
    pub fn new(layout: ManagedHeapLayout) -> Result<Self, HandleTableError> {
        Self::with_backend(layout, Box::new(PlatformHandleRegion::reserve()?))
    }

    /// 用指定后端创建 handle table（wasm 后端注入 wasmtime shared region）。
    pub fn with_backend(
        layout: ManagedHeapLayout,
        backend: Box<dyn HandleRegionBackend>,
    ) -> Result<Self, HandleTableError> {
        Ok(Self {
            layout,
            region: HandleRegion::new(backend)?,
            next_handle: AtomicU64::new(0),
            epochs: EpochQuarantine::new(),
        })
    }

    pub const fn layout(&self) -> &ManagedHeapLayout {
        &self.layout
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

    pub fn allocate_handle(&self) -> Result<HandleId, HandleTableError> {
        if let Some(handle) = self.epochs.take_reusable() {
            return Ok(handle);
        }
        let raw = self
            .next_handle
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |next| {
                (next <= u64::from(u32::MAX)).then_some(next + 1)
            })
            .map_err(|_| HandleTableError::HandleExhausted)?;
        Ok(HandleId::new(raw as u32))
    }

    pub fn publish(
        &self,
        handle: HandleId,
        address: u64,
        generation: HandleGeneration,
    ) -> Result<(), HandleTableError> {
        self.require_object_address(address)?;
        self.region.commit(handle);
        let next = ColoredHandleEntry::new(address, HandleState::stable_for(generation))?;
        self.compare_exchange(handle, HandleState::Free, next)
    }

    #[inline(always)]
    pub fn resolve(&self, handle: HandleId) -> Option<ColoredHandleEntry> {
        let entry = ColoredHandleEntry::from_raw(self.region.load_entry(handle));
        (!matches!(entry.state(), HandleState::Free | HandleState::Retired)).then_some(entry)
    }

    pub fn promote(&self, handle: HandleId) -> Result<(), HandleTableError> {
        let current =
            ColoredHandleEntry::from_raw(self.region.load_entry(handle));
        let next = ColoredHandleEntry::new(current.address(), HandleState::StableOld)?;
        self.compare_exchange(handle, HandleState::StableYoung, next)
    }

    pub fn begin_relocation(&self, handle: HandleId) -> Result<(), HandleTableError> {
        let current =
            ColoredHandleEntry::from_raw(self.region.load_entry(handle));
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
        let current =
            ColoredHandleEntry::from_raw(self.region.load_entry(handle));
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
        let current =
            ColoredHandleEntry::from_raw(self.region.load_entry(handle));
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
        self.epochs.retire(handle);
        Ok(())
    }

    pub fn register_participant(&self) -> EpochParticipant {
        self.epochs.register()
    }

    pub fn advance_epoch(&self) -> u64 {
        self.epochs.advance()
    }

    pub fn reclaim_quarantine(&self) -> usize {
        let handles = self.epochs.take_reclaimable();
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
