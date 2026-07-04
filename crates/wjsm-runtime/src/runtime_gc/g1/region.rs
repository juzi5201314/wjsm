use wjsm_ir::constants;

pub const REGION_SIZE: usize = 64 * 1024;
pub const CARD_SIZE: usize = constants::GC_CARD_SIZE as usize;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RegionKind {
    Free = 0,
    Eden = 1,
    Survivor = 2,
    Old = 3,
    HumongousStart = 4,
    HumongousCont = 5,
    Immortal = 6,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionMeta {
    pub kind: RegionKind,
    pub age: u8,
    pub implicit_black_epoch: u64,
}

impl RegionMeta {
    fn new(kind: RegionKind) -> Self {
        Self {
            kind,
            age: 0,
            implicit_black_epoch: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RegionSpace {
    object_heap_start: usize,
    dynamic_start: usize,
    meta: Vec<RegionMeta>,
}

#[allow(dead_code)]
impl RegionSpace {
    pub fn attach(
        &mut self,
        object_heap_start: usize,
        immortal_objects_end: usize,
        dynamic_start: usize,
        committed_end: usize,
    ) {
        debug_assert_eq!(REGION_SIZE % CARD_SIZE, 0);
        self.object_heap_start = object_heap_start;
        self.dynamic_start = dynamic_start;
        self.extend_for_committed_end(committed_end);
        self.mark_immortal_range(object_heap_start, immortal_objects_end);
        if let Some(idx) = self.region_index(dynamic_start)
            && self
                .meta
                .get(idx)
                .is_some_and(|region| region.kind == RegionKind::Free)
        {
            self.meta[idx] = RegionMeta::new(RegionKind::Eden);
        }
    }

    pub fn object_heap_start(&self) -> usize {
        self.object_heap_start
    }

    pub fn dynamic_start(&self) -> usize {
        self.dynamic_start
    }

    pub fn region_count(&self) -> usize {
        self.meta.len()
    }

    pub fn metadata_bytes(&self) -> usize {
        self.meta.len() * std::mem::size_of::<RegionMeta>()
    }

    pub fn region(&self, idx: usize) -> Option<&RegionMeta> {
        self.meta.get(idx)
    }

    pub fn region_index(&self, addr: usize) -> Option<usize> {
        addr.checked_sub(self.object_heap_start)
            .map(|offset| offset / REGION_SIZE)
    }

    pub fn card_index(&self, addr: usize) -> Option<usize> {
        addr.checked_sub(self.object_heap_start)
            .map(|offset| offset / CARD_SIZE)
    }

    pub fn region_start(&self, idx: usize) -> Option<usize> {
        self.object_heap_start
            .checked_add(idx.checked_mul(REGION_SIZE)?)
    }

    pub fn extend_for_committed_end(&mut self, committed_end: usize) {
        if committed_end <= self.object_heap_start {
            return;
        }
        let needed = (committed_end - self.object_heap_start).div_ceil(REGION_SIZE);
        self.meta
            .resize_with(needed, || RegionMeta::new(RegionKind::Free));
    }

    pub fn take_free_as(&mut self, kind: RegionKind) -> Option<usize> {
        let idx = self
            .meta
            .iter()
            .position(|region| region.kind == RegionKind::Free)?;
        self.meta[idx] = RegionMeta::new(kind);
        Some(idx)
    }

    pub fn release(&mut self, idx: usize) -> bool {
        let Some(region) = self.meta.get_mut(idx) else {
            return false;
        };
        *region = RegionMeta::new(RegionKind::Free);
        true
    }

    pub fn mark_humongous(&mut self, start: usize, count: usize) -> bool {
        if count == 0 || start + count > self.meta.len() {
            return false;
        }
        if self.meta[start..start + count]
            .iter()
            .any(|region| region.kind != RegionKind::Free)
        {
            return false;
        }
        self.meta[start] = RegionMeta::new(RegionKind::HumongousStart);
        for idx in start + 1..start + count {
            self.meta[idx] = RegionMeta::new(RegionKind::HumongousCont);
        }
        true
    }

    pub fn eden_window(&self) -> Option<(usize, usize)> {
        let idx = self
            .meta
            .iter()
            .position(|region| region.kind == RegionKind::Eden)?;
        let start = self.region_start(idx)?.max(self.dynamic_start);
        Some((
            start,
            start + REGION_SIZE - (start - self.region_start(idx)?),
        ))
    }

    fn mark_immortal_range(&mut self, start: usize, end: usize) {
        if end <= start {
            return;
        }
        let first = self.region_index(start).unwrap_or(0);
        let last = self.region_index(end.saturating_sub(1)).unwrap_or(first);
        for idx in first..=last {
            if let Some(region) = self.meta.get_mut(idx) {
                region.kind = RegionKind::Immortal;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_marks_immortal_and_first_eden_without_meta_region() {
        let object_heap_start = REGION_SIZE;
        let immortal_end = object_heap_start + REGION_SIZE + 1;
        let dynamic_start = object_heap_start + 2 * REGION_SIZE;
        let committed_end = object_heap_start + 4 * REGION_SIZE;
        let mut space = RegionSpace::default();

        space.attach(
            object_heap_start,
            immortal_end,
            dynamic_start,
            committed_end,
        );

        assert_eq!(space.region_count(), 4);
        assert_eq!(space.region(0).unwrap().kind, RegionKind::Immortal);
        assert_eq!(space.region(1).unwrap().kind, RegionKind::Immortal);
        assert_eq!(space.region(2).unwrap().kind, RegionKind::Eden);
        assert_eq!(space.region(3).unwrap().kind, RegionKind::Free);
        assert!(
            space
                .meta
                .iter()
                .all(|region| region.kind != RegionKind::Survivor)
        );
    }

    #[test]
    fn indices_are_based_on_object_heap_start() {
        let object_heap_start = 3 * REGION_SIZE;
        let mut space = RegionSpace::default();
        space.attach(
            object_heap_start,
            object_heap_start,
            object_heap_start,
            object_heap_start + 2 * REGION_SIZE,
        );

        assert_eq!(space.region_index(object_heap_start), Some(0));
        assert_eq!(space.region_index(object_heap_start + REGION_SIZE), Some(1));
        assert_eq!(space.card_index(object_heap_start + CARD_SIZE), Some(1));
        assert_eq!(space.region_index(object_heap_start - 1), None);
        assert_eq!(space.card_index(object_heap_start - 1), None);
    }

    #[test]
    fn grow_extends_host_metadata_only() {
        let object_heap_start = REGION_SIZE;
        let mut space = RegionSpace::default();
        space.attach(
            object_heap_start,
            object_heap_start,
            object_heap_start,
            object_heap_start + REGION_SIZE,
        );
        let initial_bytes = space.metadata_bytes();

        space.extend_for_committed_end(object_heap_start + 3 * REGION_SIZE);

        assert_eq!(space.region_count(), 3);
        assert!(space.metadata_bytes() < 8 * 1024 * 1024);
        assert!(space.metadata_bytes() > initial_bytes);
    }

    #[test]
    fn take_release_and_humongous_metadata_are_explicit() {
        let object_heap_start = REGION_SIZE;
        let mut space = RegionSpace::default();
        space.attach(
            object_heap_start,
            object_heap_start,
            object_heap_start,
            object_heap_start + 4 * REGION_SIZE,
        );

        let eden = space.take_free_as(RegionKind::Eden).unwrap();
        assert_eq!(space.region(eden).unwrap().kind, RegionKind::Eden);
        assert!(space.release(eden));
        assert_eq!(space.region(eden).unwrap().kind, RegionKind::Free);
        assert!(space.mark_humongous(eden, 2));
        assert_eq!(space.region(eden).unwrap().kind, RegionKind::HumongousStart);
        assert_eq!(
            space.region(eden + 1).unwrap().kind,
            RegionKind::HumongousCont
        );
    }
}
