use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::bitmap::AtomicBitmap;
use super::handle_entry::HandleGeneration;
use super::page::{ObjectRef, PageId, PageRange};

const OBJECT_ALIGNMENT: usize = 8;

/// 每个 page 仅保留 object-start bit；对象大小从真实 header 恢复。
pub(crate) struct ObjectMap {
    starts: AtomicBitmap,
}

impl ObjectMap {
    pub(crate) fn new(page_bytes: u64) -> Self {
        Self {
            starts: AtomicBitmap::new(page_bytes as usize / OBJECT_ALIGNMENT),
        }
    }

    pub(crate) fn record(&self, offset: u64) -> bool {
        let slot = offset as usize / OBJECT_ALIGNMENT;
        self.starts.mark(slot)
    }

    pub(crate) fn remove(&self, offset: u64) {
        self.starts.clear_bit(offset as usize / OBJECT_ALIGNMENT);
    }

    pub(crate) fn object_count(&self) -> usize {
        self.starts.count()
    }

    pub(crate) fn next_object(&self, next_slot: &mut usize, base: u64) -> Option<ObjectRef> {
        let slot = self.starts.next_set_from(*next_slot)?;
        *next_slot = slot + 1;
        Some(ObjectRef::new(base + (slot * OBJECT_ALIGNMENT) as u64))
    }

    pub(crate) fn contains(&self, offset: u64) -> bool {
        self.starts.is_marked(offset as usize / OBJECT_ALIGNMENT)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageStats {
    pub page: PageId,
    pub allocated_bytes: u64,
    pub young_live_bytes: u64,
    pub old_live_bytes: u64,
    pub object_count: usize,
    pub dedicated: bool,
}

/// 关联 page range、object-start map 与 young/old 独立 mark bitmap 的固定 metadata。
pub(crate) struct PageMetadata {
    pub(crate) range: PageRange,
    pub(crate) base_offset: u64,
    object_map: ObjectMap,
    allocated_bytes: AtomicU64,
    young_live_bytes: AtomicU64,
    old_live_bytes: AtomicU64,
    young_mark: AtomicBitmap,
    old_mark: AtomicBitmap,
    dedicated: bool,
}

impl PageMetadata {
    pub(crate) fn new(
        range: PageRange,
        page_bytes: u64,
        object_heap_base: u64,
        dedicated: bool,
    ) -> Self {
        let bits = page_bytes as usize / OBJECT_ALIGNMENT;
        Self {
            range,
            base_offset: object_heap_base + range.start().get() as u64 * page_bytes,
            object_map: ObjectMap::new(page_bytes),
            allocated_bytes: AtomicU64::new(0),
            young_live_bytes: AtomicU64::new(0),
            old_live_bytes: AtomicU64::new(0),
            young_mark: AtomicBitmap::new(bits),
            old_mark: AtomicBitmap::new(bits),
            dedicated,
        }
    }

    pub(crate) fn record(&self, object: ObjectRef, bytes: u64) -> bool {
        if !self.object_map.record(object.offset() - self.base_offset) {
            return false;
        }
        self.allocated_bytes.fetch_add(bytes, Ordering::Relaxed);
        true
    }
    pub(crate) fn forget(&self, object: ObjectRef, bytes: u64) {
        let slot = self.slot(object);
        self.object_map.remove(object.offset() - self.base_offset);
        if self.young_mark.is_marked(slot) {
            self.young_mark.clear_bit(slot);
            self.young_live_bytes.fetch_sub(bytes, Ordering::Relaxed);
        }
        if self.old_mark.is_marked(slot) {
            self.old_mark.clear_bit(slot);
            self.old_live_bytes.fetch_sub(bytes, Ordering::Relaxed);
        }
        self.allocated_bytes.fetch_sub(bytes, Ordering::Relaxed);
    }

    pub(crate) fn contains(&self, object: ObjectRef) -> bool {
        self.object_map.contains(object.offset() - self.base_offset)
    }
    pub(crate) fn object_count(&self) -> usize {
        self.object_map.object_count()
    }

    pub(crate) fn clear_marks(&self, generation: HandleGeneration) {
        let (bitmap, live_bytes) = self.generation_metadata(generation);
        bitmap.clear();
        live_bytes.store(0, Ordering::Release);
    }

    pub(crate) fn try_mark(
        &self,
        object: ObjectRef,
        bytes: u64,
        generation: HandleGeneration,
    ) -> bool {
        let (bitmap, live_bytes) = self.generation_metadata(generation);
        if !bitmap.mark(self.slot(object)) {
            return false;
        }
        live_bytes.fetch_add(bytes, Ordering::Relaxed);
        true
    }

    pub(crate) fn is_marked(&self, object: ObjectRef, generation: HandleGeneration) -> bool {
        self.generation_metadata(generation)
            .0
            .is_marked(self.slot(object))
    }

    pub(crate) fn stats(&self) -> PageStats {
        PageStats {
            page: self.range.start(),
            allocated_bytes: self.allocated_bytes.load(Ordering::Relaxed),
            young_live_bytes: self.young_live_bytes.load(Ordering::Relaxed),
            old_live_bytes: self.old_live_bytes.load(Ordering::Relaxed),
            object_count: self.object_count(),
            dedicated: self.dedicated,
        }
    }

    fn slot(&self, object: ObjectRef) -> usize {
        ((object.offset() - self.base_offset) / OBJECT_ALIGNMENT as u64) as usize
    }

    fn generation_metadata(&self, generation: HandleGeneration) -> (&AtomicBitmap, &AtomicU64) {
        match generation {
            HandleGeneration::Young => (&self.young_mark, &self.young_live_bytes),
            HandleGeneration::Old => (&self.old_mark, &self.old_live_bytes),
        }
    }
}

/// 按 object-start bit streaming 遍历 page，而不构造对象列表。
pub struct PageObjectIter {
    page: Option<Arc<PageMetadata>>,
    next_slot: usize,
}

impl PageObjectIter {
    pub(crate) fn new(page: Option<Arc<PageMetadata>>) -> Self {
        Self { page, next_slot: 0 }
    }
}

impl Iterator for PageObjectIter {
    type Item = ObjectRef;

    fn next(&mut self) -> Option<Self::Item> {
        let page = self.page.as_ref()?;
        page.object_map
            .next_object(&mut self.next_slot, page.base_offset)
    }
}
