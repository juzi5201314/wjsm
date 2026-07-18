use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

use super::ManagedHeapLayout;
use super::object_map::{PageMetadata, PageObjectIter};
use super::page::{AllocationClass, ObjectRef, PageConfig, PageId, PageRange};

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

    fn try_allocate(&mut self, bytes: u64, allocated_bytes: &AtomicU64) -> Option<Allocation> {
        let page = self.page.as_ref()?;
        let end = self.top.checked_add(bytes)?;
        if end > self.end {
            return None;
        }
        let object = ObjectRef::new(self.top);
        page.record(object, bytes);
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

/// relocation 专用 page 区间；mutator free list 不会持有它。
#[derive(Clone, Copy, Debug)]
pub struct RelocationReserve {
    pages: PageRange,
}

impl RelocationReserve {
    pub const fn pages(&self) -> PageRange {
        self.pages
    }
}

/// page/NLAB 分配前台；慢路径才进入 `state` mutex。
pub struct ManagedAllocator {
    layout: ManagedHeapLayout,
    config: PageConfig,
    total_pages: u32,
    state: Mutex<AllocatorState>,
    allocated_bytes: AtomicU64,
    committed_bytes: AtomicU64,
}

impl ManagedAllocator {
    pub fn new(layout: ManagedHeapLayout) -> Result<Self, AllocatorError> {
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
            state: Mutex::new(AllocatorState::new(total_pages)),
            allocated_bytes: AtomicU64::new(0),
            committed_bytes: AtomicU64::new(0),
        })
    }

    pub const fn layout(&self) -> &ManagedHeapLayout {
        &self.layout
    }

    pub fn allocate(&self, nlab: &mut Nlab, bytes: u64) -> Result<Allocation, AllocatorError> {
        let bytes = align_object_size(bytes)?;
        let class = self.class_for(bytes);
        if class == AllocationClass::Small {
            if let Some(allocation) = nlab.try_allocate(bytes, &self.allocated_bytes) {
                return Ok(allocation);
            }
            let page = self.acquire_pages(1)?;
            nlab.install(page, self.config.bytes);
            return nlab
                .try_allocate(bytes, &self.allocated_bytes)
                .ok_or(AllocatorError::NlabRefillTooSmall { bytes });
        }
        self.allocate_dedicated(class, bytes)
    }

    pub fn reserve_relocation(&self, pages: u32) -> Result<RelocationReserve, AllocatorError> {
        let mut state = self.state.lock();
        let range = state.take_free(pages)?;
        state.reserves.insert(range.start().get(), range.len());
        Ok(RelocationReserve { pages: range })
    }

    pub fn release_relocation(&self, reserve: RelocationReserve) -> Result<(), AllocatorError> {
        let mut state = self.state.lock();
        let length = state
            .reserves
            .remove(&reserve.pages.start().get())
            .ok_or(AllocatorError::UnknownRelocationReserve)?;
        if length != reserve.pages.len() {
            return Err(AllocatorError::UnknownRelocationReserve);
        }
        state.free.insert(reserve.pages);
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

    pub fn forget_object(&self, object: ObjectRef, bytes: u64) -> Result<(), AllocatorError> {
        self.metadata_for_object(object)?.forget(object);
        self.allocated_bytes.fetch_sub(bytes, Ordering::Relaxed);
        Ok(())
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

    pub fn clear_current_marks(&self) {
        let state = self.state.lock();
        for (page_id, metadata) in &state.pages {
            if *page_id == metadata.range.start() {
                metadata.clear_current_marks();
            }
        }
    }

    pub fn mark_current(&self, object: ObjectRef) -> Result<(), AllocatorError> {
        self.metadata_for_object(object)?.mark_current(object);
        Ok(())
    }

    pub fn mark_previous(&self, object: ObjectRef) -> Result<(), AllocatorError> {
        self.metadata_for_object(object)?.mark_previous(object);
        Ok(())
    }

    pub fn is_marked_current(&self, object: ObjectRef) -> Result<bool, AllocatorError> {
        Ok(self.metadata_for_object(object)?.is_marked_current(object))
    }

    pub fn is_marked_previous(&self, object: ObjectRef) -> Result<bool, AllocatorError> {
        Ok(self.metadata_for_object(object)?.is_marked_previous(object))
    }

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
        let page = self.acquire_pages(page_count)?;
        let object = ObjectRef::new(page.base_offset);
        page.record(object, bytes);
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

    fn acquire_pages(&self, count: u32) -> Result<Arc<PageMetadata>, AllocatorError> {
        let mut state = self.state.lock();
        let range = state.take_free(count)?;
        let page = Arc::new(PageMetadata::new(
            range,
            self.config.bytes,
            self.layout.object_heap_base(),
        ));
        let newly_committed = state.commit(range);
        state.insert_page(Arc::clone(&page));
        self.committed_bytes.fetch_add(
            newly_committed as u64 * self.config.bytes,
            Ordering::Relaxed,
        );
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
        if let Some((&previous_start, &previous_length)) = self.ranges.range(..start).next_back() {
            if previous_start + previous_length == start {
                start = previous_start;
                self.ranges.remove(&previous_start);
            }
        }
        if let Some((&next_start, &next_length)) = self.ranges.range(end..).next() {
            if next_start == end {
                end += next_length;
                self.ranges.remove(&next_start);
            }
        }
        self.ranges.insert(start, end - start);
    }

    fn page_count(&self) -> u32 {
        self.ranges.values().sum()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AllocatorError {
    InvalidLayout(&'static str),
    NlabRefillTooSmall { bytes: u64 },
    OutOfPages { requested: u32, available: u32 },
    RequestTooLarge { bytes: u64 },
    SharedNlabAllocation,
    UnknownObject { object: ObjectRef },
    UnknownPage { page: PageId },
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
            Self::SharedNlabAllocation => {
                formatter.write_str("cannot release an individual NLAB object")
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
