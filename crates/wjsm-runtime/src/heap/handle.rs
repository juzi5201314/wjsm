use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use wasmtime::{Engine, MemoryType, SharedMemory};
use wjsm_engine_config::{EngineConfig, RuntimeEngineOptions};

use super::epoch::{EpochParticipant, EpochQuarantine};
use super::handle_entry::{
    ADDRESS_LIMIT, ColoredHandleEntry, HANDLE_ENTRY_BYTES, HANDLE_REGION_BYTES,
    HEAP_COMMIT_GRANULE_BYTES, HandleGeneration, HandleId, HandleState, HandleTableError,
};

const HANDLE_BLOCK_ENTRIES: usize = (HEAP_COMMIT_GRANULE_BYTES / HANDLE_ENTRY_BYTES) as usize;
const HANDLE_REGION_PAGES: u64 = HANDLE_REGION_BYTES / HEAP_COMMIT_GRANULE_BYTES;
const COMMIT_BITMAP_WORDS: usize = (HANDLE_REGION_PAGES / u64::BITS as u64) as usize;

/// memory64 中的固定 reserve/control/object 地址布局。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedHeapLayout {
    control_base: u64,
    control_end: u64,
    object_heap_base: u64,
    object_heap_end: u64,
}

impl ManagedHeapLayout {
    pub fn new(max_heap_size: u64, control_reserved: u64) -> Result<Self, HandleTableError> {
        let control_base = HANDLE_REGION_BYTES;
        let control_end = align_up(
            control_base
                .checked_add(control_reserved)
                .ok_or(HandleTableError::LayoutOverflow)?,
        )?;
        let object_heap_end = control_end
            .checked_add(max_heap_size)
            .ok_or(HandleTableError::LayoutOverflow)?;
        if object_heap_end > ADDRESS_LIMIT {
            return Err(HandleTableError::LayoutExceedsAddressSpace { object_heap_end });
        }
        Ok(Self {
            control_base,
            control_end,
            object_heap_base: control_end,
            object_heap_end,
        })
    }

    pub const fn control_base(&self) -> u64 {
        self.control_base
    }

    pub const fn control_end(&self) -> u64 {
        self.control_end
    }

    pub const fn object_heap_base(&self) -> u64 {
        self.object_heap_base
    }

    pub const fn object_heap_end(&self) -> u64 {
        self.object_heap_end
    }

    fn contains_object_address(&self, address: u64) -> bool {
        (self.object_heap_base..self.object_heap_end).contains(&address)
    }
}

fn align_up(value: u64) -> Result<u64, HandleTableError> {
    let remainder = value % HEAP_COMMIT_GRANULE_BYTES;
    if remainder == 0 {
        Ok(value)
    } else {
        value
            .checked_add(HEAP_COMMIT_GRANULE_BYTES - remainder)
            .ok_or(HandleTableError::LayoutOverflow)
    }
}

/// V2 的连续 memory64 handle region；仅第一次发布 block 时增加 committed 计数。
struct HandleRegion {
    _engine: Engine,
    _memory: SharedMemory,
    base: usize,
    committed_blocks: Box<[AtomicU64]>,
    committed_bytes: AtomicU64,
}

impl HandleRegion {
    fn reserve() -> Result<Self, HandleTableError> {
        let engine = EngineConfig::runtime(RuntimeEngineOptions {
            memory_reservation: Some(HANDLE_REGION_BYTES),
            ..RuntimeEngineOptions::default()
        })
        .build()
        .map_err(reservation_error)?;
        let memory_ty = MemoryType::builder()
            .memory64(true)
            .shared(true)
            .min(HANDLE_REGION_PAGES)
            .max(Some(HANDLE_REGION_PAGES))
            .build()
            .map_err(reservation_error)?;
        let memory = SharedMemory::new(&engine, memory_ty).map_err(reservation_error)?;
        if memory.data_size() as u64 != HANDLE_REGION_BYTES {
            return Err(HandleTableError::VirtualReservation {
                detail: format!(
                    "expected {HANDLE_REGION_BYTES} bytes, got {}",
                    memory.data_size()
                ),
            });
        }
        let base = memory.data().as_ptr().cast::<u8>() as usize;
        if base == 0 {
            return Err(HandleTableError::VirtualReservation {
                detail: "Wasmtime returned a null shared-memory base".to_owned(),
            });
        }
        let committed_blocks = std::iter::repeat_with(|| AtomicU64::new(0))
            .take(COMMIT_BITMAP_WORDS)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Ok(Self {
            _engine: engine,
            _memory: memory,
            base,
            committed_blocks,
            committed_bytes: AtomicU64::new(0),
        })
    }

    #[inline(always)]
    fn entry(&self, handle: HandleId) -> &AtomicU64 {
        let offset = HandleTableV2::entry_address(handle) as usize;
        // SAFETY: memory64 min=max 固定为 32 GiB，`_memory` 保持 mapping 存活；
        // handle offset 落在该范围且按 8-byte entry 对齐。该 region 只以 AtomicU64 访问。
        unsafe { AtomicU64::from_ptr((self.base + offset) as *mut u64) }
    }

    fn commit(&self, handle: HandleId) {
        let block = handle.get() as usize / HANDLE_BLOCK_ENTRIES;
        let word = block / u64::BITS as usize;
        let bit = 1_u64 << (block % u64::BITS as usize);
        let previous = self.committed_blocks[word].fetch_or(bit, Ordering::SeqCst);
        if previous & bit == 0 {
            self.committed_bytes
                .fetch_add(HEAP_COMMIT_GRANULE_BYTES, Ordering::SeqCst);
        }
    }

    fn committed_bytes(&self) -> u64 {
        self.committed_bytes.load(Ordering::SeqCst)
    }
}

fn reservation_error(error: impl std::fmt::Display) -> HandleTableError {
    HandleTableError::VirtualReservation {
        detail: error.to_string(),
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
    pub fn new(layout: ManagedHeapLayout) -> Result<Self, HandleTableError> {
        Ok(Self {
            layout,
            region: HandleRegion::reserve()?,
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
        let entry = ColoredHandleEntry::from_raw(self.region.entry(handle).load(Ordering::SeqCst));
        (!matches!(entry.state(), HandleState::Free | HandleState::Retired)).then_some(entry)
    }

    pub fn promote(&self, handle: HandleId) -> Result<(), HandleTableError> {
        let current =
            ColoredHandleEntry::from_raw(self.region.entry(handle).load(Ordering::SeqCst));
        let next = ColoredHandleEntry::new(current.address(), HandleState::StableOld)?;
        self.compare_exchange(handle, HandleState::StableYoung, next)
    }

    pub fn begin_relocation(&self, handle: HandleId) -> Result<(), HandleTableError> {
        let current =
            ColoredHandleEntry::from_raw(self.region.entry(handle).load(Ordering::SeqCst));
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
            ColoredHandleEntry::from_raw(self.region.entry(handle).load(Ordering::SeqCst));
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
            ColoredHandleEntry::from_raw(self.region.entry(handle).load(Ordering::SeqCst));
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
