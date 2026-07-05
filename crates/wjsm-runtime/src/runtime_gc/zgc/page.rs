#![allow(dead_code)] // T4.1 建立 T4.2-T4.4 会接入的 page metadata API。
use super::color::{ZColor, ZEntry};

pub const ZPAGE_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ZPageKind {
    Free = 0,
    Active = 1,
    Relocating = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZPageMeta {
    pub kind: ZPageKind,
    pub live_bytes: usize,
    pub relocation_set: bool,
    pub age: u8,
}

impl ZPageMeta {
    fn new(kind: ZPageKind) -> Self {
        Self {
            kind,
            live_bytes: 0,
            relocation_set: false,
            age: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadHandleCleanupStage {
    WeakAndSideTables,
    PublishHandles,
}

pub const DEAD_HANDLE_CLEANUP_ORDER: [DeadHandleCleanupStage; 2] = [
    DeadHandleCleanupStage::WeakAndSideTables,
    DeadHandleCleanupStage::PublishHandles,
];

#[derive(Debug, Clone, Default)]
pub struct ZPageSpace {
    dynamic_start: usize,
    pages: Vec<ZPageMeta>,
}

impl ZPageSpace {
    pub fn attach(&mut self, dynamic_start: usize, committed_end: usize) {
        self.dynamic_start = dynamic_start;
        self.extend_for_committed_end(committed_end);
    }

    pub fn dynamic_start(&self) -> usize {
        self.dynamic_start
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn metadata_bytes(&self) -> usize {
        self.pages.len() * std::mem::size_of::<ZPageMeta>()
    }

    pub fn page(&self, idx: usize) -> Option<&ZPageMeta> {
        self.pages.get(idx)
    }

    pub fn page_index(&self, addr: usize) -> Option<usize> {
        addr.checked_sub(self.dynamic_start)
            .map(|offset| offset / ZPAGE_SIZE)
    }

    pub fn page_start(&self, idx: usize) -> Option<usize> {
        self.dynamic_start.checked_add(idx.checked_mul(ZPAGE_SIZE)?)
    }

    pub fn extend_for_committed_end(&mut self, committed_end: usize) {
        if committed_end <= self.dynamic_start {
            return;
        }
        let needed = (committed_end - self.dynamic_start).div_ceil(ZPAGE_SIZE);
        self.pages
            .resize_with(needed, || ZPageMeta::new(ZPageKind::Free));
    }

    pub fn reset_live_bytes(&mut self) {
        for page in &mut self.pages {
            page.live_bytes = 0;
        }
    }

    pub fn set_live_bytes(&mut self, idx: usize, bytes: usize) {
        if let Some(page) = self.pages.get_mut(idx) {
            page.live_bytes = bytes;
            if bytes != 0 && page.kind == ZPageKind::Free {
                page.kind = ZPageKind::Active;
            }
        }
    }

    pub fn mark_live_bytes(&mut self, ptr: usize, bytes: usize) -> Option<()> {
        let idx = self.page_index(ptr)?;
        let page = self.pages.get_mut(idx)?;
        page.kind = ZPageKind::Active;
        page.live_bytes = page.live_bytes.saturating_add(bytes);
        Some(())
    }

    pub fn mark_relocation_set(&mut self, idx: usize) -> bool {
        let Some(page) = self.pages.get_mut(idx) else {
            return false;
        };
        if page.kind == ZPageKind::Free {
            return false;
        }
        page.kind = ZPageKind::Relocating;
        page.relocation_set = true;
        true
    }

    pub fn reclaim_dead_pages(&mut self) -> Vec<usize> {
        let mut reclaimed = Vec::new();
        for (idx, page) in self.pages.iter_mut().enumerate() {
            if page.kind != ZPageKind::Free && page.live_bytes == 0 {
                *page = ZPageMeta::new(ZPageKind::Free);
                reclaimed.push(idx);
            }
        }
        reclaimed
    }
}

pub fn recolor_live_obj_table_entries(
    data: &mut [u8],
    obj_table_ptr: usize,
    count: usize,
    good: ZColor,
) -> usize {
    let mut recolored = 0;
    for handle in 0..count {
        let slot = obj_table_ptr + handle * 4;
        let Some(bytes) = data.get_mut(slot..slot + 4) else {
            break;
        };
        let mut raw = [0u8; 4];
        raw.copy_from_slice(bytes);
        let entry = ZEntry::from(u32::from_le_bytes(raw));
        if entry.is_empty() {
            continue;
        }
        bytes.copy_from_slice(&entry.recolor(good).raw().to_le_bytes());
        recolored += 1;
    }
    recolored
}

impl From<u32> for ZEntry {
    fn from(raw: u32) -> Self {
        if raw == 0 {
            Self::empty()
        } else {
            Self::new(
                raw & super::color::PTR_MASK,
                ZColor::from_bits(raw).unwrap_or(ZColor::Empty),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEAD_HANDLE_CLEANUP_ORDER, DeadHandleCleanupStage, ZPAGE_SIZE, ZPageKind, ZPageSpace,
        recolor_live_obj_table_entries,
    };
    use crate::runtime_gc::zgc::color::{ZColor, ZEntry};

    #[test]
    fn attach_grows_host_side_page_metadata() {
        let mut space = ZPageSpace::default();
        let dynamic_start = 2 * ZPAGE_SIZE;

        space.attach(dynamic_start, dynamic_start + 3 * ZPAGE_SIZE);

        assert_eq!(space.dynamic_start(), dynamic_start);
        assert_eq!(space.page_count(), 3);
        assert!(space.metadata_bytes() < 8 * 1024 * 1024);
        assert_eq!(space.page_index(dynamic_start + ZPAGE_SIZE), Some(1));
        assert_eq!(space.page_index(dynamic_start - 1), None);
    }

    #[test]
    fn attach_recolors_all_live_obj_table_entries_non_empty() {
        let mut data = vec![0u8; 32];
        data[0..4].copy_from_slice(&ZEntry::new(0x1000, ZColor::Empty).raw().to_le_bytes());
        data[4..8].copy_from_slice(&0u32.to_le_bytes());
        data[8..12].copy_from_slice(&ZEntry::new(0x2000, ZColor::Marked0).raw().to_le_bytes());

        let recolored = recolor_live_obj_table_entries(&mut data, 0, 3, ZColor::Marked1);

        assert_eq!(recolored, 2);
        assert_eq!(
            ZEntry::from(u32::from_le_bytes(data[0..4].try_into().unwrap())).color(),
            ZColor::Marked1
        );
        assert_eq!(u32::from_le_bytes(data[4..8].try_into().unwrap()), 0);
        assert_eq!(
            ZEntry::from(u32::from_le_bytes(data[8..12].try_into().unwrap())).color(),
            ZColor::Marked1
        );
    }

    #[test]
    fn relocating_page_uses_remapped_good_color() {
        let mut entry = ZEntry::new(0x4000, ZColor::Marked0);

        entry = entry.repair_relocate_non_rs();

        assert_eq!(entry.color(), ZColor::Remapped);
    }

    #[test]
    fn dead_pages_reclaim_immediately() {
        let mut space = ZPageSpace::default();
        space.attach(0, 3 * ZPAGE_SIZE);
        space.mark_live_bytes(0, 0).unwrap();
        space.mark_live_bytes(ZPAGE_SIZE, 128).unwrap();
        assert!(space.mark_relocation_set(0));
        assert!(space.mark_relocation_set(1));

        let reclaimed = space.reclaim_dead_pages();

        assert_eq!(reclaimed, vec![0]);
        assert_eq!(space.page(0).unwrap().kind, ZPageKind::Free);
        assert_eq!(space.page(1).unwrap().kind, ZPageKind::Relocating);
    }

    #[test]
    fn weak_cleanup_precedes_handle_reuse_protocol() {
        assert_eq!(
            DEAD_HANDLE_CLEANUP_ORDER,
            [
                DeadHandleCleanupStage::WeakAndSideTables,
                DeadHandleCleanupStage::PublishHandles,
            ]
        );
    }
}
