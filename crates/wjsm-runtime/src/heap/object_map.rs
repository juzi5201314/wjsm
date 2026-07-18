use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::bitmap::AtomicBitmap;
use super::page::{ObjectRef, PageRange};

const OBJECT_ALIGNMENT: usize = 8;

/// 每个 page 的 object-start bit 与 size table；创建 page 时一次性分配。
pub(crate) struct ObjectMap {
    starts: AtomicBitmap,
    sizes: Box<[AtomicU64]>,
}

impl ObjectMap {
    pub(crate) fn new(page_bytes: u64) -> Self {
        let slots = page_bytes as usize / OBJECT_ALIGNMENT;
        let sizes = std::iter::repeat_with(|| AtomicU64::new(0))
            .take(slots)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            starts: AtomicBitmap::new(slots),
            sizes,
        }
    }

    pub(crate) fn record(&self, offset: u64, bytes: u64) {
        let slot = offset as usize / OBJECT_ALIGNMENT;
        debug_assert!(slot < self.sizes.len());
        self.sizes[slot].store(bytes, Ordering::Release);
        self.starts.mark(slot);
    }

    pub(crate) fn remove(&self, offset: u64) {
        let slot = offset as usize / OBJECT_ALIGNMENT;
        debug_assert!(slot < self.sizes.len());
        self.sizes[slot].store(0, Ordering::Release);
        self.starts.clear_bit(slot);
    }

    pub(crate) fn object_count(&self) -> usize {
        self.starts.count()
    }

    pub(crate) fn next_object(&self, next_slot: &mut usize, base: u64) -> Option<ObjectRef> {
        let slot = self.starts.next_set_from(*next_slot)?;
        debug_assert_ne!(self.sizes[slot].load(Ordering::Acquire), 0);
        *next_slot = slot + 1;
        Some(ObjectRef::new(base + (slot * OBJECT_ALIGNMENT) as u64))
    }
}

/// 关联 page range、object map 和双 mark bitmap 的固定 metadata。
pub(crate) struct PageMetadata {
    pub(crate) range: PageRange,
    pub(crate) base_offset: u64,
    object_map: ObjectMap,
    current_mark: AtomicBitmap,
    previous_mark: AtomicBitmap,
}

impl PageMetadata {
    pub(crate) fn new(range: PageRange, page_bytes: u64, object_heap_base: u64) -> Self {
        let bits = page_bytes as usize / OBJECT_ALIGNMENT;
        Self {
            range,
            base_offset: object_heap_base + range.start().get() as u64 * page_bytes,
            object_map: ObjectMap::new(page_bytes),
            current_mark: AtomicBitmap::new(bits),
            previous_mark: AtomicBitmap::new(bits),
        }
    }

    pub(crate) fn record(&self, object: ObjectRef, bytes: u64) {
        self.object_map
            .record(object.offset() - self.base_offset, bytes);
    }

    pub(crate) fn forget(&self, object: ObjectRef) {
        let slot = ((object.offset() - self.base_offset) / OBJECT_ALIGNMENT as u64) as usize;
        self.object_map.remove(object.offset() - self.base_offset);
        self.current_mark.clear_bit(slot);
        self.previous_mark.clear_bit(slot);
    }

    pub(crate) fn object_count(&self) -> usize {
        self.object_map.object_count()
    }

    pub(crate) fn clear_current_marks(&self) {
        self.current_mark.clear();
    }

    pub(crate) fn mark_current(&self, object: ObjectRef) {
        self.current_mark
            .mark(((object.offset() - self.base_offset) / OBJECT_ALIGNMENT as u64) as usize);
    }

    pub(crate) fn mark_previous(&self, object: ObjectRef) {
        self.previous_mark
            .mark(((object.offset() - self.base_offset) / OBJECT_ALIGNMENT as u64) as usize);
    }

    pub(crate) fn is_marked_current(&self, object: ObjectRef) -> bool {
        self.current_mark
            .is_marked(((object.offset() - self.base_offset) / OBJECT_ALIGNMENT as u64) as usize)
    }

    pub(crate) fn is_marked_previous(&self, object: ObjectRef) -> bool {
        self.previous_mark
            .is_marked(((object.offset() - self.base_offset) / OBJECT_ALIGNMENT as u64) as usize)
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
