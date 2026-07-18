use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use crate::heap::PageId;

const MIXED_LIVE_PERCENT: u64 = 85;

impl G1V2 {
    pub fn collect_young(
        &self,
        roots: &RootSnapshot,
        mut cleanup: impl FnMut(HandleId),
    ) -> Result<G1V2Report, G1V2Error> {
        let started = Instant::now();
        self.reset_nlab();
        let (dirty_cards, remembered_roots) = self.remembered_young_roots();
        let objects_snapshot = self.objects.lock().clone();
        let mut roots = snapshot_handles(roots);
        roots.extend(remembered_roots);
        let reachable = mark_reachable(&objects_snapshot, roots);
        self.mark_object_map(&objects_snapshot, &reachable)?;
        let dead = objects_snapshot
            .iter()
            .filter_map(|(&handle, object)| {
                (object.generation.is_young() && !reachable.contains(&handle)).then_some(handle)
            })
            .collect::<Vec<_>>();
        let live = objects_snapshot
            .iter()
            .filter_map(|(&handle, object)| {
                (object.generation.is_young() && reachable.contains(&handle)).then_some(handle)
            })
            .collect::<Vec<_>>();
        let mut report = G1V2Report {
            kind: Some(G1V2CollectionKind::Young),
            marked: reachable.len(),
            remembered_cards_scanned: dirty_cards.len(),
            ..G1V2Report::default()
        };
        self.retire_objects(&dead, &mut report, &mut cleanup)?;
        self.evacuate_young(&live, &mut report)?;
        self.rebuild_remembered_set();
        self.record_collection(&report, started.elapsed());
        Ok(report)
    }

    pub fn collect_mixed(
        &self,
        roots: &RootSnapshot,
        mut cleanup: impl FnMut(HandleId),
    ) -> Result<G1V2Report, G1V2Error> {
        let started = Instant::now();
        self.reset_nlab();
        let snapshot = self.objects.lock().clone();
        let reachable = mark_reachable(&snapshot, snapshot_handles(roots));
        self.mark_object_map(&snapshot, &reachable)?;
        let selected_pages = mixed_pages(&snapshot, &reachable);
        let dead = snapshot
            .iter()
            .filter_map(|(&handle, object)| {
                (!object.generation.is_young() && !reachable.contains(&handle)).then_some(handle)
            })
            .collect::<Vec<_>>();
        let live = snapshot
            .iter()
            .filter_map(|(&handle, object)| {
                (!object.generation.is_young()
                    && reachable.contains(&handle)
                    && selected_pages.contains(&object.allocation.page()))
                .then_some(handle)
            })
            .collect::<Vec<_>>();
        let mut report = G1V2Report {
            kind: Some(G1V2CollectionKind::Mixed),
            marked: reachable.len(),
            ..G1V2Report::default()
        };
        self.retire_objects(&dead, &mut report, &mut cleanup)?;
        self.evacuate_old(&live, &mut report)?;
        self.rebuild_remembered_set();
        self.record_collection(&report, started.elapsed());
        Ok(report)
    }

    pub fn collect_full(
        &self,
        roots: &RootSnapshot,
        mut cleanup: impl FnMut(HandleId),
    ) -> Result<G1V2Report, G1V2Error> {
        let started = Instant::now();
        self.reset_nlab();
        let snapshot = self.objects.lock().clone();
        let reachable = mark_reachable(&snapshot, snapshot_handles(roots));
        self.mark_object_map(&snapshot, &reachable)?;
        let dead = snapshot
            .keys()
            .filter(|handle| !reachable.contains(handle))
            .copied()
            .collect::<Vec<_>>();
        let mut report = G1V2Report {
            kind: Some(G1V2CollectionKind::Full),
            marked: reachable.len(),
            ..G1V2Report::default()
        };
        self.retire_objects(&dead, &mut report, &mut cleanup)?;
        self.rebuild_remembered_set();
        self.record_collection(&report, started.elapsed());
        Ok(report)
    }

    fn remembered_young_roots(&self) -> (Vec<usize>, BTreeSet<HandleId>) {
        let dirty_cards = self.rset.lock().dirty_card_snapshot();
        let dirty = dirty_cards.iter().copied().collect::<BTreeSet<_>>();
        let objects = self.objects.lock();
        let mut roots = BTreeSet::new();
        for object in objects
            .values()
            .filter(|object| !object.generation.is_young())
        {
            for (slot, target) in object.references.iter().enumerate() {
                let address = object.allocation.object().offset() + slot as u64 * 8;
                let card = self.card_index(address);
                if dirty.contains(&card)
                    && let Some(target) = target
                    && objects
                        .get(target)
                        .is_some_and(|target| target.generation.is_young())
                {
                    roots.insert(*target);
                }
            }
        }
        (dirty_cards, roots)
    }

    fn rebuild_remembered_set(&self) {
        let objects = self.objects.lock();
        let mut rset = self.rset.lock();
        for card in rset.dirty_card_snapshot() {
            rset.clear_card(card);
        }
        for object in objects
            .values()
            .filter(|object| !object.generation.is_young())
        {
            for (slot, target) in object.references.iter().enumerate() {
                let Some(target) = target else {
                    continue;
                };
                if !objects
                    .get(target)
                    .is_some_and(|target| target.generation.is_young())
                {
                    continue;
                }
                let address = object.allocation.object().offset() + slot as u64 * 8;
                rset.mark_dirty_slot(address as usize, self.card_index(address));
            }
        }
    }

    fn mark_object_map(
        &self,
        objects: &BTreeMap<HandleId, G1Object>,
        reachable: &BTreeSet<HandleId>,
    ) -> Result<(), G1V2Error> {
        self.heap.allocator().clear_current_marks();
        for handle in reachable {
            if let Some(object) = objects.get(handle) {
                self.heap
                    .allocator()
                    .mark_current(object.allocation.object())?;
            }
        }
        Ok(())
    }

    fn reset_nlab(&self) {
        *self.nlab.lock() = Nlab::new();
    }
}

fn snapshot_handles(snapshot: &RootSnapshot) -> BTreeSet<HandleId> {
    snapshot
        .handles()
        .iter()
        .copied()
        .map(HandleId::new)
        .collect()
}

fn mark_reachable(
    objects: &BTreeMap<HandleId, G1Object>,
    roots: BTreeSet<HandleId>,
) -> BTreeSet<HandleId> {
    let mut marked = BTreeSet::new();
    let mut pending = roots;
    while let Some(handle) = pending.pop_first() {
        let Some(object) = objects.get(&handle) else {
            continue;
        };
        if !marked.insert(handle) {
            continue;
        }
        pending.extend(
            object
                .references
                .iter()
                .flatten()
                .copied()
                .filter(|handle| !marked.contains(handle)),
        );
    }
    marked
}

fn mixed_pages(
    objects: &BTreeMap<HandleId, G1Object>,
    reachable: &BTreeSet<HandleId>,
) -> BTreeSet<PageId> {
    let mut pages = BTreeMap::<PageId, (u64, u64)>::new();
    for (handle, object) in objects {
        if object.generation.is_young() || object.allocation.is_dedicated() {
            continue;
        }
        let entry = pages.entry(object.allocation.page()).or_default();
        entry.0 += object.allocation.bytes();
        if reachable.contains(handle) {
            entry.1 += object.allocation.bytes();
        }
    }
    pages
        .into_iter()
        .filter_map(|(page, (total, live))| {
            (live < total && live * 100 <= total * MIXED_LIVE_PERCENT).then_some(page)
        })
        .collect()
}
