use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::ManagedHeapLayout;
use super::epoch::HeapEpoch;
use super::handle::HandleRangeReservation;
use parking_lot::Mutex;

use super::handle_entry::HandleGeneration;
use super::memory::HeapMemory;
use super::object_map::{PageMetadata, PageObjectIter, PageStats};
use super::page::{AllocationClass, ObjectRef, PageConfig, PageId, PageRange};
use super::word::{HeapAddress, HeapMemoryError};

const OBJECT_ALIGNMENT: u64 = 8;

/// V2 allocator 的一次对象分配结果。
#[derive(Clone, Debug)]
pub struct Allocation {
    object: ObjectRef,
    page: PageId,
    pages: PageRange,
    class: AllocationClass,
    dedicated: bool,
    bytes: u64,
}

impl Allocation {
    pub const fn object(&self) -> ObjectRef {
        self.object
    }

    pub const fn page(&self) -> PageId {
        self.page
    }

    pub const fn pages(&self) -> PageRange {
        self.pages
    }

    pub const fn class(&self) -> AllocationClass {
        self.class
    }

    pub const fn is_dedicated(&self) -> bool {
        self.dedicated
    }

    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// 由 mutator 独占的 local allocation buffer；命中路径不获取 allocator lock。
pub struct Nlab {
    page: Option<Arc<PageMetadata>>,
    top: u64,
    end: u64,
    refills: u64,
}

impl Nlab {
    pub const fn new() -> Self {
        Self {
            page: None,
            top: 0,
            end: 0,
            refills: 0,
        }
    }

    pub const fn refills(&self) -> u64 {
        self.refills
    }

    pub fn reset(&mut self) {
        self.page = None;
        self.top = 0;
        self.end = 0;
    }

    fn try_allocate(&mut self, bytes: u64, allocated_bytes: &AtomicU64) -> Option<Allocation> {
        let page = self.page.as_ref()?;
        let end = self.top.checked_add(bytes)?;
        if end > self.end {
            return None;
        }
        let object = ObjectRef::new(self.top);
        if !page.record(object, bytes) {
            return None;
        }
        self.top = end;
        allocated_bytes.fetch_add(bytes, Ordering::Relaxed);
        Some(Allocation {
            object,
            page: page.range.start(),
            pages: page.range,
            class: AllocationClass::Small,
            dedicated: false,
            bytes,
        })
    }

    fn install(&mut self, page: Arc<PageMetadata>, page_bytes: u64) {
        self.top = page.base_offset;
        self.end = page.base_offset + page_bytes;
        self.page = Some(page);
        self.refills += 1;
    }
}

impl Default for Nlab {
    fn default() -> Self {
        Self::new()
    }
}

/// Native code 可直接消费的单页 Small-object reservation。
///
/// reservation 只在 flush/materialize 时更新 page object-start metadata；生成代码消费
/// `object_start..object_limit` 与 `handle_start..handle_limit` 内的连续游标，不能把
/// reusable handle 或尚未 materialize 的对象暴露给 collector。
pub struct NativeTlabReservation {
    page: Arc<PageMetadata>,
    object_start: u64,
    object_limit: u64,
    small_object_limit: u64,
    handles: HandleRangeReservation,
    materialized_top: u64,
    materialized_handles: u32,
    zeroed: bool,
}

impl NativeTlabReservation {
    pub const fn object_start(&self) -> u64 {
        self.object_start
    }

    pub const fn object_limit(&self) -> u64 {
        self.object_limit
    }

    pub const fn handle_start(&self) -> u32 {
        self.handles.start()
    }

    pub const fn handle_limit(&self) -> u32 {
        self.handles.limit()
    }
    pub const fn handle_range(&self) -> HandleRangeReservation {
        self.handles
    }

    pub fn page_range(&self) -> PageRange {
        self.page.range
    }

    pub const fn is_zeroed(&self) -> bool {
        self.zeroed
    }

    pub const fn materialized_top(&self) -> u64 {
        self.materialized_top
    }

    pub const fn materialized_handles(&self) -> u32 {
        self.materialized_handles
    }

    pub const fn small_object_limit(&self) -> u64 {
        self.small_object_limit
    }

    /// 在 reservation 重新绑定前清零整个范围；新提交页本身也经此路径建立显式保证。
    pub fn zero_range<M: HeapMemory>(&mut self, memory: &M) -> Result<(), AllocatorError> {
        let mut address = self.object_start;
        let mut remaining = self.object_limit - self.object_start;
        const ZERO_CHUNK: [u8; 4096] = [0; 4096];
        while remaining != 0 {
            let length = remaining.min(ZERO_CHUNK.len() as u64) as usize;
            memory
                .copy_from(HeapAddress::new(address), &ZERO_CHUNK[..length])
                .map_err(AllocatorError::NativeTlabZeroing)?;
            address += length as u64;
            remaining -= length as u64;
        }
        self.zeroed = true;
        Ok(())
    }

    /// 把尚未登记的 object header 区间 materialize 到 page metadata。
    ///
    /// `read_object_size` 必须读取当前 header、校验其 heap type/capacity/handle，并返回
    /// 已按 8 字节对齐的完整对象大小；allocator 只负责校验连续边界、handle 数量和
    /// object-start/allocated-byte/mark 元数据发布。
    pub fn materialize_native_tlab(
        &mut self,
        top: u64,
        next_handle: u32,
        mut read_object_size: impl FnMut(u64, u32) -> Result<u64, AllocatorError>,
        allocator: &ManagedAllocator,
        mark_generation: Option<HandleGeneration>,
    ) -> Result<(), AllocatorError> {
        if !self.zeroed {
            return Err(AllocatorError::NativeTlabNotZeroed);
        }
        if top < self.materialized_top || top > self.object_limit {
            return Err(AllocatorError::NativeTlabTopOutOfBounds {
                top,
                start: self.materialized_top,
                limit: self.object_limit,
            });
        }
        if next_handle < self.materialized_handles || next_handle > self.handle_limit() {
            return Err(AllocatorError::NativeTlabHandleOutOfBounds {
                next_handle,
                start: self.materialized_handles,
                limit: self.handle_limit(),
            });
        }
        if self.materialized_top < top && next_handle == self.materialized_handles.saturating_add(1)
        {
            let object = self.materialized_top;
            let bytes = read_object_size(object, self.materialized_handles)?;
            let end = object
                .checked_add(bytes)
                .ok_or(AllocatorError::NativeTlabInvalidObject { object, bytes })?;
            if bytes < u64::from(wjsm_ir::constants::HEAP_OBJECT_HEADER_SIZE)
                || bytes > self.small_object_limit
                || !bytes.is_multiple_of(OBJECT_ALIGNMENT)
                || end != top
                || end > self.object_limit
            {
                return Err(AllocatorError::NativeTlabInvalidObject { object, bytes });
            }
            if !self.page.record(ObjectRef::new(object), bytes) {
                return Err(AllocatorError::DuplicateObject {
                    object: ObjectRef::new(object),
                });
            }
            if let Some(generation) = mark_generation {
                self.page
                    .try_mark(ObjectRef::new(object), bytes, generation);
            }
            allocator
                .allocated_bytes
                .fetch_add(bytes, Ordering::Relaxed);
            self.materialized_top = top;
            self.materialized_handles = next_handle;
            return Ok(());
        }
        let mut objects = Vec::with_capacity((next_handle - self.materialized_handles) as usize);
        let mut object = self.materialized_top;
        let mut handle = self.materialized_handles;
        while object < top {
            let bytes = read_object_size(object, handle)?;
            if bytes < u64::from(wjsm_ir::constants::HEAP_OBJECT_HEADER_SIZE)
                || bytes > self.small_object_limit
                || !bytes.is_multiple_of(OBJECT_ALIGNMENT)
            {
                return Err(AllocatorError::NativeTlabInvalidObject { object, bytes });
            }
            let end = object
                .checked_add(bytes)
                .ok_or(AllocatorError::NativeTlabInvalidObject { object, bytes })?;
            if end > top || end > self.object_limit {
                return Err(AllocatorError::NativeTlabInvalidObject { object, bytes });
            }
            objects.push((ObjectRef::new(object), bytes));
            object = end;
            handle = handle
                .checked_add(1)
                .ok_or(AllocatorError::NativeTlabHandleOutOfBounds {
                    next_handle,
                    start: self.materialized_handles,
                    limit: self.handle_limit(),
                })?;
        }
        if object != top || handle != next_handle {
            return Err(AllocatorError::NativeTlabHandleCountMismatch {
                expected: handle,
                actual: next_handle,
            });
        }
        let total_bytes = objects
            .iter()
            .try_fold(0_u64, |total, (_, bytes)| total.checked_add(*bytes));
        let Some(total_bytes) = total_bytes else {
            return Err(AllocatorError::RequestTooLarge { bytes: top });
        };
        for (object, bytes) in &objects {
            if !bytes.is_multiple_of(OBJECT_ALIGNMENT) {
                return Err(AllocatorError::NativeTlabInvalidObject {
                    object: object.offset(),
                    bytes: *bytes,
                });
            }
        }
        for (object, bytes) in objects {
            if !self.page.record(object, bytes) {
                return Err(AllocatorError::DuplicateObject { object });
            }
            if let Some(generation) = mark_generation {
                self.page.try_mark(object, bytes, generation);
            }
        }
        allocator
            .allocated_bytes
            .fetch_add(total_bytes, Ordering::Relaxed);
        self.materialized_top = top;
        self.materialized_handles = next_handle;
        Ok(())
    }
}

impl ManagedAllocator {
    /// 使用 allocator 的真实 page/class 判定建立单页 native reservation。
    pub fn reserve_native_tlab(
        &self,
        handles: HandleRangeReservation,
    ) -> Result<NativeTlabReservation, AllocatorError> {
        if handles.start() >= handles.limit() {
            return Err(AllocatorError::NativeTlabHandleOutOfBounds {
                next_handle: handles.start(),
                start: handles.start(),
                limit: handles.limit(),
            });
        }
        let page = self.acquire_pages(1, false)?;
        let object_start = page.base_offset;
        let object_limit =
            object_start
                .checked_add(self.config.bytes)
                .ok_or(AllocatorError::RequestTooLarge {
                    bytes: self.config.bytes,
                })?;
        Ok(NativeTlabReservation {
            page,
            object_start,
            object_limit,
            small_object_limit: self.config.small_limit,
            handles,
            materialized_top: object_start,
            materialized_handles: handles.start(),
            zeroed: false,
        })
    }

    pub fn allocation_class(&self, bytes: u64) -> Result<AllocationClass, AllocatorError> {
        let bytes = align_object_size(bytes)?;
        Ok(self.class_for(bytes))
    }
    /// 生成代码使用的 Small class 上界；该值来自 allocator 的 PageConfig 唯一判定。
    pub const fn small_object_limit(&self) -> u64 {
        self.config.small_limit
    }
}

/// relocation 专用 page 区间；mutator free set 永远不能取得其中页面。
pub struct RelocationNlab {
    pages: PageRange,
    next_page: u32,
    current: Nlab,
}

impl RelocationNlab {
    pub const fn pages(&self) -> PageRange {
        self.pages
    }

    pub const fn remaining_pages(&self) -> u32 {
        self.pages
            .start()
            .get()
            .saturating_add(self.pages.len())
            .saturating_sub(self.next_page)
    }
}

/// page/NLAB 分配前台；慢路径才进入 `state` mutex。
pub struct ManagedAllocator {
    layout: ManagedHeapLayout,
    config: PageConfig,
    total_pages: u32,
    epoch: Arc<HeapEpoch>,
    state: Mutex<AllocatorState>,
    allocated_bytes: AtomicU64,
    committed_bytes: AtomicU64,
}

impl ManagedAllocator {
    pub fn new(layout: ManagedHeapLayout) -> Result<Self, AllocatorError> {
        Self::with_epoch(layout, HeapEpoch::new())
    }

    pub fn with_epoch(
        layout: ManagedHeapLayout,
        epoch: Arc<HeapEpoch>,
    ) -> Result<Self, AllocatorError> {
        let heap_bytes = layout.object_heap_end() - layout.object_heap_base();
        let config = PageConfig::for_heap(heap_bytes).map_err(AllocatorError::InvalidLayout)?;
        let total_pages = heap_bytes / config.bytes;
        let total_pages = u32::try_from(total_pages)
            .map_err(|_| AllocatorError::InvalidLayout("page count exceeds u32"))?;
        if total_pages == 0 {
            return Err(AllocatorError::InvalidLayout(
                "heap has no allocatable pages",
            ));
        }
        Ok(Self {
            layout,
            config,
            total_pages,
            epoch,
            state: Mutex::new(AllocatorState::new(total_pages)),
            allocated_bytes: AtomicU64::new(0),
            committed_bytes: AtomicU64::new(0),
        })
    }

    pub const fn layout(&self) -> &ManagedHeapLayout {
        &self.layout
    }

    pub fn epoch(&self) -> Arc<HeapEpoch> {
        Arc::clone(&self.epoch)
    }

    pub fn quarantine_allocation(&self, object: ObjectRef, bytes: u64) {
        self.epoch.retire_allocation(object.offset(), bytes);
    }

    pub fn take_reclaimable_allocations(&self) -> Vec<(u64, u64)> {
        self.epoch.take_reclaimable_allocations()
    }

    pub fn allocate(&self, nlab: &mut Nlab, bytes: u64) -> Result<Allocation, AllocatorError> {
        let bytes = align_object_size(bytes)?;
        let class = self.class_for(bytes);
        if class == AllocationClass::Small {
            if let Some(allocation) = nlab.try_allocate(bytes, &self.allocated_bytes) {
                return Ok(allocation);
            }
            let page = self.acquire_pages(1, false)?;
            nlab.install(page, self.config.bytes);
            return nlab
                .try_allocate(bytes, &self.allocated_bytes)
                .ok_or(AllocatorError::NlabRefillTooSmall { bytes });
        }
        self.allocate_dedicated(class, bytes)
    }

    pub fn restore_object(&self, object: ObjectRef, bytes: u64) -> Result<(), AllocatorError> {
        let bytes = align_object_size(bytes)?;
        let relative = object
            .offset()
            .checked_sub(self.layout.object_heap_base())
            .ok_or(AllocatorError::UnknownObject { object })?;
        let offset_in_page = relative % self.config.bytes;
        let first_page = u32::try_from(relative / self.config.bytes)
            .map_err(|_| AllocatorError::UnknownObject { object })?;
        let page_count = u32::try_from((offset_in_page + bytes).div_ceil(self.config.bytes))
            .map_err(|_| AllocatorError::RequestTooLarge { bytes })?;
        let range = PageRange::new(PageId::new(first_page), page_count);
        if first_page
            .checked_add(page_count)
            .is_none_or(|end| end > self.total_pages)
        {
            return Err(AllocatorError::UnknownObject { object });
        }

        let metadata = {
            let mut state = self.state.lock();
            if let Some(metadata) = state.pages.get(&range.start()).cloned() {
                if metadata.range != range {
                    return Err(AllocatorError::RestorePageUnavailable {
                        page: range.start(),
                    });
                }
                metadata
            } else {
                state
                    .free
                    .take_range(range)
                    .ok_or(AllocatorError::RestorePageUnavailable {
                        page: range.start(),
                    })?;
                let metadata = Arc::new(PageMetadata::new(
                    range,
                    self.config.bytes,
                    self.layout.object_heap_base(),
                    page_count > 1 || self.class_for(bytes) != AllocationClass::Small,
                ));
                let newly_committed = state.commit(range);
                state.insert_page(Arc::clone(&metadata));
                self.committed_bytes.fetch_add(
                    u64::from(newly_committed) * self.config.bytes,
                    Ordering::Relaxed,
                );
                metadata
            }
        };
        if !metadata.record(object, bytes) {
            return Err(AllocatorError::DuplicateObject { object });
        }
        self.allocated_bytes.fetch_add(bytes, Ordering::Relaxed);
        Ok(())
    }

    pub fn reserve_relocation(&self, pages: u32) -> Result<RelocationNlab, AllocatorError> {
        let mut state = self.state.lock();
        let range = state.take_free(pages)?;
        state.reserves.insert(range.start().get(), range.len());
        Ok(RelocationNlab {
            pages: range,
            next_page: range.start().get(),
            current: Nlab::new(),
        })
    }

    pub fn allocate_relocation(
        &self,
        nlab: &mut RelocationNlab,
        bytes: u64,
    ) -> Result<Allocation, AllocatorError> {
        let bytes = align_object_size(bytes)?;
        let class = self.class_for(bytes);
        if class == AllocationClass::Small {
            if let Some(allocation) = nlab.current.try_allocate(bytes, &self.allocated_bytes) {
                return Ok(allocation);
            }
            let page = self.acquire_relocation_pages(nlab, 1, false)?;
            nlab.current.install(page, self.config.bytes);
            return nlab
                .current
                .try_allocate(bytes, &self.allocated_bytes)
                .ok_or(AllocatorError::NlabRefillTooSmall { bytes });
        }
        let page_count = u32::try_from(bytes.div_ceil(self.config.bytes))
            .map_err(|_| AllocatorError::RequestTooLarge { bytes })?;
        let page = self.acquire_relocation_pages(nlab, page_count, true)?;
        let object = ObjectRef::new(page.base_offset);
        if !page.record(object, bytes) {
            return Err(AllocatorError::DuplicateObject { object });
        }
        self.allocated_bytes.fetch_add(bytes, Ordering::Relaxed);
        Ok(Allocation {
            object,
            page: page.range.start(),
            pages: page.range,
            class,
            dedicated: true,
            bytes,
        })
    }

    pub fn finish_relocation(&self, mut nlab: RelocationNlab) -> Result<(), AllocatorError> {
        nlab.current.reset();
        let mut state = self.state.lock();
        let length = state
            .reserves
            .remove(&nlab.pages.start().get())
            .ok_or(AllocatorError::UnknownRelocationReserve)?;
        if length != nlab.pages.len() {
            return Err(AllocatorError::UnknownRelocationReserve);
        }
        let end = nlab.pages.start().get() + nlab.pages.len();
        if nlab.next_page < end {
            state.free.insert(PageRange::new(
                PageId::new(nlab.next_page),
                end - nlab.next_page,
            ));
        }
        Ok(())
    }

    pub fn release_dedicated(&self, allocation: &Allocation) -> Result<(), AllocatorError> {
        if !allocation.dedicated {
            return Err(AllocatorError::SharedNlabAllocation);
        }
        let mut state = self.state.lock();
        let metadata = state
            .pages
            .get(&allocation.page)
            .ok_or(AllocatorError::UnknownPage {
                page: allocation.page,
            })?;
        if metadata.range != allocation.pages {
            return Err(AllocatorError::UnknownPage {
                page: allocation.page,
            });
        }
        state.remove_range(allocation.pages);
        state.free.insert(allocation.pages);
        self.allocated_bytes
            .fetch_sub(allocation.bytes, Ordering::Relaxed);
        Ok(())
    }

    /// 对象是否已登记到 page object-start metadata，可由 collector 与 relocation 观察。
    pub fn contains_object(&self, object: ObjectRef) -> bool {
        self.metadata_for_object(object)
            .is_ok_and(|metadata| metadata.contains(object))
    }

    pub fn forget_object(&self, object: ObjectRef, bytes: u64) -> Result<(), AllocatorError> {
        self.metadata_for_object(object)?.forget(object, bytes);
        self.allocated_bytes.fetch_sub(bytes, Ordering::Relaxed);
        Ok(())
    }
    pub fn forget_object_if_present(
        &self,
        object: ObjectRef,
        bytes: u64,
    ) -> Result<bool, AllocatorError> {
        let Ok(metadata) = self.metadata_for_object(object) else {
            return Ok(false);
        };
        if !metadata.contains(object) {
            return Ok(false);
        }
        metadata.forget(object, bytes);
        self.allocated_bytes.fetch_sub(bytes, Ordering::Relaxed);
        Ok(true)
    }

    /// 从 object-start map 删除对象；整段 page 为空时归还 mutator free set。
    pub fn reclaim_object(&self, object: ObjectRef, bytes: u64) -> Result<u32, AllocatorError> {
        let metadata = self.metadata_for_object(object)?;
        metadata.forget(object, bytes);
        self.allocated_bytes.fetch_sub(bytes, Ordering::Relaxed);
        if metadata.object_count() != 0 {
            return Ok(0);
        }
        let mut state = self.state.lock();
        if metadata.object_count() != 0 {
            return Ok(0);
        }
        state.remove_range(metadata.range);
        state.free.insert(metadata.range);
        Ok(metadata.range.len())
    }
    pub fn reclaim_quarantined_object(
        &self,
        object: ObjectRef,
        bytes: u64,
    ) -> Result<u32, AllocatorError> {
        let Ok(metadata) = self.metadata_for_object(object) else {
            return Ok(0);
        };
        if metadata.contains(object) {
            return self.reclaim_object(object, bytes);
        }
        if metadata.object_count() != 0 {
            return Ok(0);
        }
        let mut state = self.state.lock();
        if metadata.object_count() != 0 || !state.pages.contains_key(&metadata.range.start()) {
            return Ok(0);
        }
        state.remove_range(metadata.range);
        state.free.insert(metadata.range);
        Ok(metadata.range.len())
    }

    pub fn release_empty_page(&self, page: PageId) -> Result<bool, AllocatorError> {
        let mut state = self.state.lock();
        let Some(metadata) = state.pages.get(&page).cloned() else {
            return Err(AllocatorError::UnknownPage { page });
        };
        if metadata.range.len() != 1 || metadata.object_count() != 0 {
            return Ok(false);
        }
        state.remove_range(metadata.range);
        state.free.insert(metadata.range);
        Ok(true)
    }
    /// 回收尚未登记任何对象的 TLAB 页面；页面已被 collector 移除时返回 false。
    pub fn release_empty_page_if_present(&self, page: PageId) -> bool {
        let mut state = self.state.lock();
        let Some(metadata) = state.pages.get(&page).cloned() else {
            return false;
        };
        if metadata.range.len() != 1 || metadata.object_count() != 0 {
            return false;
        }
        state.remove_range(metadata.range);
        state.free.insert(metadata.range);
        true
    }

    pub fn clear_marks(&self, generation: HandleGeneration) {
        let state = self.state.lock();
        for (page_id, metadata) in &state.pages {
            if *page_id == metadata.range.start() {
                metadata.clear_marks(generation);
            }
        }
    }

    pub fn try_mark(
        &self,
        object: ObjectRef,
        bytes: u64,
        generation: HandleGeneration,
    ) -> Result<bool, AllocatorError> {
        Ok(self
            .metadata_for_object(object)?
            .try_mark(object, bytes, generation))
    }

    pub fn is_marked(
        &self,
        object: ObjectRef,
        generation: HandleGeneration,
    ) -> Result<bool, AllocatorError> {
        Ok(self
            .metadata_for_object(object)?
            .is_marked(object, generation))
    }

    pub fn transfer_mark(
        &self,
        source: ObjectRef,
        destination: ObjectRef,
        destination_bytes: u64,
        generation: HandleGeneration,
    ) -> Result<(), AllocatorError> {
        if self.is_marked(source, generation)? {
            self.try_mark(destination, destination_bytes, generation)?;
        }
        Ok(())
    }

    pub fn page_stats(&self) -> Vec<PageStats> {
        let state = self.state.lock();
        state
            .pages
            .iter()
            .filter(|(page, metadata)| **page == metadata.range.start())
            .map(|(_, metadata)| metadata.stats())
            .collect()
    }
    /// 遍历已登记到 page object-start metadata 的对象。
    ///
    /// 调用方（collector、relocation、handles_in_page）必须在 safepoint 后调用，
    /// 即 native TLAB 已通过 `materialize_native_tlab` 把全部已分配对象登记完毕。
    /// 未物化的 TLAB 对象不应出现在 page 迭代结果中——它们的 metadata 尚未发布。
    #[inline]
    pub fn objects_in_page(&self, page: PageId) -> PageObjectIter {
        let page = self.state.lock().pages.get(&page).cloned();
        PageObjectIter::new(page)
    }

    pub fn object_count(&self, page: PageId) -> usize {
        self.state
            .lock()
            .pages
            .get(&page)
            .map_or(0, |metadata| metadata.object_count())
    }

    pub fn pages_are_contiguous(&self, range: PageRange) -> bool {
        let state = self.state.lock();
        (0..range.len()).all(|offset| {
            state
                .pages
                .get(&PageId::new(range.start().get() + offset))
                .is_some_and(|metadata| metadata.range == range)
        })
    }

    pub fn free_pages(&self) -> u32 {
        self.state.lock().free.page_count()
    }

    pub fn free_bytes(&self) -> u64 {
        u64::from(self.free_pages()) * self.config.bytes
    }

    pub const fn total_pages(&self) -> u32 {
        self.total_pages
    }

    pub fn committed_bytes(&self) -> u64 {
        self.committed_bytes.load(Ordering::Relaxed)
    }

    pub fn allocated_bytes(&self) -> u64 {
        self.allocated_bytes.load(Ordering::Relaxed)
    }

    fn allocate_dedicated(
        &self,
        class: AllocationClass,
        bytes: u64,
    ) -> Result<Allocation, AllocatorError> {
        let page_count = bytes.div_ceil(self.config.bytes);
        let page_count =
            u32::try_from(page_count).map_err(|_| AllocatorError::RequestTooLarge { bytes })?;
        let page = self.acquire_pages(page_count, true)?;
        let object = ObjectRef::new(page.base_offset);
        if !page.record(object, bytes) {
            return Err(AllocatorError::DuplicateObject { object });
        }
        self.allocated_bytes.fetch_add(bytes, Ordering::Relaxed);
        Ok(Allocation {
            object,
            page: page.range.start(),
            pages: page.range,
            class,
            dedicated: true,
            bytes,
        })
    }

    fn acquire_pages(
        &self,
        count: u32,
        dedicated: bool,
    ) -> Result<Arc<PageMetadata>, AllocatorError> {
        let mut state = self.state.lock();
        let range = state.take_free(count)?;
        let page = Arc::new(PageMetadata::new(
            range,
            self.config.bytes,
            self.layout.object_heap_base(),
            dedicated,
        ));
        let newly_committed = state.commit(range);
        state.insert_page(Arc::clone(&page));
        self.committed_bytes.fetch_add(
            newly_committed as u64 * self.config.bytes,
            Ordering::Relaxed,
        );
        Ok(page)
    }

    fn acquire_relocation_pages(
        &self,
        nlab: &mut RelocationNlab,
        count: u32,
        dedicated: bool,
    ) -> Result<Arc<PageMetadata>, AllocatorError> {
        let reserve_end = nlab.pages.start().get() + nlab.pages.len();
        let end = nlab
            .next_page
            .checked_add(count)
            .ok_or(AllocatorError::RequestTooLarge {
                bytes: u64::from(count) * self.config.bytes,
            })?;
        if end > reserve_end {
            return Err(AllocatorError::OutOfPages {
                requested: count,
                available: nlab.remaining_pages(),
            });
        }
        let range = PageRange::new(PageId::new(nlab.next_page), count);
        let mut state = self.state.lock();
        if state.reserves.get(&nlab.pages.start().get()) != Some(&nlab.pages.len()) {
            return Err(AllocatorError::UnknownRelocationReserve);
        }
        let page = Arc::new(PageMetadata::new(
            range,
            self.config.bytes,
            self.layout.object_heap_base(),
            dedicated,
        ));
        let newly_committed = state.commit(range);
        state.insert_page(Arc::clone(&page));
        self.committed_bytes.fetch_add(
            u64::from(newly_committed) * self.config.bytes,
            Ordering::Relaxed,
        );
        nlab.next_page = end;
        Ok(page)
    }

    fn metadata_for_object(&self, object: ObjectRef) -> Result<Arc<PageMetadata>, AllocatorError> {
        let relative = object
            .offset()
            .checked_sub(self.layout.object_heap_base())
            .ok_or(AllocatorError::UnknownObject { object })?;
        let raw_page = relative / self.config.bytes;
        let page = u32::try_from(raw_page)
            .map(PageId::new)
            .map_err(|_| AllocatorError::UnknownObject { object })?;
        self.state
            .lock()
            .pages
            .get(&page)
            .cloned()
            .ok_or(AllocatorError::UnknownObject { object })
    }

    fn class_for(&self, bytes: u64) -> AllocationClass {
        if bytes <= self.config.small_limit {
            AllocationClass::Small
        } else if bytes <= self.config.medium_limit {
            AllocationClass::Medium
        } else if bytes <= self.config.large_limit {
            AllocationClass::Large
        } else {
            AllocationClass::Humongous
        }
    }
}

fn align_object_size(bytes: u64) -> Result<u64, AllocatorError> {
    if bytes == 0 {
        return Err(AllocatorError::ZeroSizedObject);
    }
    bytes
        .checked_add(OBJECT_ALIGNMENT - 1)
        .map(|value| value & !(OBJECT_ALIGNMENT - 1))
        .ok_or(AllocatorError::RequestTooLarge { bytes })
}

struct AllocatorState {
    free: FreePageRanges,
    committed: Vec<bool>,
    pages: BTreeMap<PageId, Arc<PageMetadata>>,
    reserves: BTreeMap<u32, u32>,
}

impl AllocatorState {
    fn new(total_pages: u32) -> Self {
        Self {
            free: FreePageRanges::new(total_pages),
            committed: vec![false; total_pages as usize],
            pages: BTreeMap::new(),
            reserves: BTreeMap::new(),
        }
    }

    fn take_free(&mut self, count: u32) -> Result<PageRange, AllocatorError> {
        self.free.take(count).ok_or(AllocatorError::OutOfPages {
            requested: count,
            available: self.free.page_count(),
        })
    }

    fn commit(&mut self, range: PageRange) -> u32 {
        let mut newly_committed = 0;
        for page in range.start().get()..range.start().get() + range.len() {
            let committed = &mut self.committed[page as usize];
            if !*committed {
                *committed = true;
                newly_committed += 1;
            }
        }
        newly_committed
    }

    fn insert_page(&mut self, page: Arc<PageMetadata>) {
        for offset in 0..page.range.len() {
            self.pages.insert(
                PageId::new(page.range.start().get() + offset),
                Arc::clone(&page),
            );
        }
    }

    fn remove_range(&mut self, range: PageRange) {
        for offset in 0..range.len() {
            self.pages
                .remove(&PageId::new(range.start().get() + offset));
        }
    }
}

struct FreePageRanges {
    ranges: BTreeMap<u32, u32>,
}

impl FreePageRanges {
    fn new(total_pages: u32) -> Self {
        Self {
            ranges: BTreeMap::from([(0, total_pages)]),
        }
    }

    fn take(&mut self, count: u32) -> Option<PageRange> {
        let (start, length) = self
            .ranges
            .iter()
            .find_map(|(&start, &length)| (length >= count).then_some((start, length)))?;
        self.ranges.remove(&start);
        if length > count {
            self.ranges.insert(start + count, length - count);
        }
        Some(PageRange::new(PageId::new(start), count))
    }

    fn insert(&mut self, range: PageRange) {
        let mut start = range.start().get();
        let mut end = start + range.len();
        if let Some((&previous_start, &previous_length)) = self.ranges.range(..start).next_back()
            && previous_start + previous_length == start
        {
            start = previous_start;
            self.ranges.remove(&previous_start);
        }
        if let Some((&next_start, &next_length)) = self.ranges.range(end..).next()
            && next_start == end
        {
            end += next_length;
            self.ranges.remove(&next_start);
        }
        self.ranges.insert(start, end - start);
    }

    fn take_range(&mut self, range: PageRange) -> Option<()> {
        let requested_start = range.start().get();
        let requested_end = requested_start.checked_add(range.len())?;
        let (&free_start, &free_len) = self.ranges.range(..=requested_start).next_back()?;
        let free_end = free_start.checked_add(free_len)?;
        if requested_end > free_end {
            return None;
        }
        self.ranges.remove(&free_start);
        if free_start < requested_start {
            self.ranges.insert(free_start, requested_start - free_start);
        }
        if requested_end < free_end {
            self.ranges.insert(requested_end, free_end - requested_end);
        }
        Some(())
    }

    fn page_count(&self) -> u32 {
        self.ranges.values().sum()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AllocatorError {
    DuplicateObject {
        object: ObjectRef,
    },
    InvalidLayout(&'static str),
    NlabRefillTooSmall {
        bytes: u64,
    },
    NativeTlabHandleCountMismatch {
        expected: u32,
        actual: u32,
    },
    NativeTlabHandleOutOfBounds {
        next_handle: u32,
        start: u32,
        limit: u32,
    },
    NativeTlabInvalidHeader {
        object: u64,
        detail: String,
    },
    NativeTlabInvalidObject {
        object: u64,
        bytes: u64,
    },
    NativeTlabNotZeroed,
    NativeTlabTopOutOfBounds {
        top: u64,
        start: u64,
        limit: u64,
    },
    NativeTlabZeroing(HeapMemoryError),
    OutOfPages {
        requested: u32,
        available: u32,
    },
    RequestTooLarge {
        bytes: u64,
    },
    RestorePageUnavailable {
        page: PageId,
    },
    SharedNlabAllocation,
    UnknownObject {
        object: ObjectRef,
    },
    UnknownPage {
        page: PageId,
    },
    UnknownRelocationReserve,
    ZeroSizedObject,
}

impl fmt::Display for AllocatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLayout(reason) => {
                write!(formatter, "invalid managed heap layout: {reason}")
            }
            Self::NlabRefillTooSmall { bytes } => {
                write!(formatter, "NLAB cannot fit {bytes} bytes")
            }
            Self::NativeTlabHandleCountMismatch { expected, actual } => write!(
                formatter,
                "native TLAB materialized handle count {expected} does not match cursor {actual}"
            ),
            Self::NativeTlabHandleOutOfBounds {
                next_handle,
                start,
                limit,
            } => write!(
                formatter,
                "native TLAB handle cursor {next_handle} is outside [{start}, {limit})"
            ),
            Self::NativeTlabInvalidHeader { object, detail } => {
                write!(
                    formatter,
                    "invalid native TLAB object header at {object}: {detail}"
                )
            }
            Self::NativeTlabInvalidObject { object, bytes } => {
                write!(
                    formatter,
                    "invalid native TLAB object at {object} with size {bytes}"
                )
            }
            Self::NativeTlabNotZeroed => formatter.write_str("native TLAB range is not zeroed"),
            Self::NativeTlabZeroing(error) => {
                write!(formatter, "native TLAB zeroing failed: {error}")
            }
            Self::NativeTlabTopOutOfBounds { top, start, limit } => write!(
                formatter,
                "native TLAB top {top} is outside [{start}, {limit}]"
            ),
            Self::OutOfPages {
                requested,
                available,
            } => write!(
                formatter,
                "requested {requested} pages with only {available} free"
            ),
            Self::RequestTooLarge { bytes } => {
                write!(formatter, "object request {bytes} bytes is too large")
            }
            Self::DuplicateObject { object } => write!(
                formatter,
                "duplicate heap object at offset {}",
                object.offset()
            ),
            Self::SharedNlabAllocation => {
                formatter.write_str("cannot release an individual NLAB object")
            }
            Self::RestorePageUnavailable { page } => {
                write!(
                    formatter,
                    "snapshot page {} is already allocated",
                    page.get()
                )
            }
            Self::UnknownObject { object } => write!(
                formatter,
                "unknown heap object at offset {}",
                object.offset()
            ),
            Self::UnknownPage { page } => write!(formatter, "unknown page {}", page.get()),
            Self::UnknownRelocationReserve => formatter.write_str("unknown relocation reserve"),
            Self::ZeroSizedObject => formatter.write_str("zero-sized objects are invalid"),
        }
    }
}

impl Error for AllocatorError {}
