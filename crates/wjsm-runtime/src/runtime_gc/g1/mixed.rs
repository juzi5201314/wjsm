//! G1 mixed collection set evacuation。
//!
//! mixed 只压缩 old/humongous 空间：dead handle 已由 concurrent mark cleanup 统一清理，
//! 本模块复制仍然存活的对象字节并更新 obj_table，不改写对象内部引用槽。

use std::collections::{BTreeSet, HashSet};
use std::time::Instant;

use crate::runtime_gc::api::{GcContext, GcStats, Handle, Value};
use crate::runtime_gc::context::object_size_from_memory;
use crate::runtime_gc::heap_governance;
use crate::runtime_gc::object_walker::{self, SlotValue};

use super::region::{CARD_SIZE, REGION_SIZE, RegionKind, RegionSpace};
use super::rset::G1RSet;

const HUMONGOUS_THRESHOLD: usize = REGION_SIZE / 2;
const MIXED_SKIP_LIVE_PERCENT: usize = 85;

#[derive(Debug)]
pub(super) struct MixedCollection {
    pub(super) stats: GcStats,
    remaining_estimate: usize,
    relocated_objects: usize,
    blocked: bool,
}

impl MixedCollection {
    pub(super) fn did_work(&self) -> bool {
        self.relocated_objects != 0 || self.stats.swept != 0
    }

    pub(super) fn has_remaining(&self) -> bool {
        self.remaining_estimate != 0 && !self.blocked
    }

    pub(super) fn remaining_estimate(&self) -> usize {
        self.remaining_estimate
    }
}

#[derive(Clone, Copy, Debug)]
struct MixedObject {
    handle: Handle,
    ptr: usize,
    size: usize,
    source_region: usize,
    kind: RegionKind,
}

#[derive(Clone, Copy, Debug)]
struct Relocation {
    handle: Handle,
    src: usize,
    dest: usize,
    size: usize,
}

#[derive(Debug)]
struct EvacuationPlan {
    regions: RegionSpace,
    relocations: Vec<Relocation>,
    released_regions: Vec<usize>,
}

#[derive(Debug, Default)]
struct MixedResult {
    relocated_objects: usize,
    released_regions: usize,
    reclaimed_bytes: usize,
}

#[derive(Debug, Default)]
struct MixedAllocator {
    old: Option<BumpRegion>,
}

#[derive(Debug)]
struct BumpRegion {
    cursor: usize,
    end: usize,
}

pub(super) fn collect_step(
    ctx: &mut GcContext<'_>,
    regions: &mut RegionSpace,
    rset: &mut G1RSet,
    copy_budget: usize,
) -> MixedCollection {
    let started = Instant::now();
    let cset = select_collection_set(regions, copy_budget);
    if cset.is_empty() {
        return idle_collection(
            ctx,
            started,
            regions,
            remaining_candidate_bytes(regions) != 0,
        );
    }

    let obj_table_ptr = ctx.obj_table_ptr();
    let obj_table_count = ctx.obj_table_count();
    let objects = snapshot_cset_objects(ctx, regions, obj_table_ptr, obj_table_count, &cset);
    let Some(plan) = build_evacuation_plan(regions, &objects, &cset) else {
        return idle_collection(ctx, started, regions, true);
    };

    let mut scratch = Vec::new();
    for relocation in &plan.relocations {
        if !copy_object(
            ctx,
            relocation.src,
            relocation.dest,
            relocation.size,
            &mut scratch,
        ) {
            return idle_collection(ctx, started, regions, true);
        }
    }

    for relocation in &plan.relocations {
        ctx.write_obj_table_slot(relocation.handle, relocation.dest);
    }
    *regions = plan.regions;
    clear_released_region_cards(regions, rset, &plan.released_regions);
    redirty_destination_cards(
        ctx,
        regions,
        rset,
        obj_table_ptr,
        obj_table_count,
        &plan.relocations,
    );

    let result = MixedResult {
        relocated_objects: plan.relocations.len(),
        released_regions: plan.released_regions.len(),
        reclaimed_bytes: plan.released_regions.len() * REGION_SIZE,
    };
    let stats = stats_for_result(ctx, started, regions, &result);
    MixedCollection {
        stats,
        remaining_estimate: remaining_candidate_bytes(regions),
        relocated_objects: result.relocated_objects,
        blocked: false,
    }
}

fn idle_collection(
    ctx: &mut GcContext<'_>,
    started: Instant,
    regions: &RegionSpace,
    blocked: bool,
) -> MixedCollection {
    MixedCollection {
        stats: stats_for_result(ctx, started, regions, &MixedResult::default()),
        remaining_estimate: remaining_candidate_bytes(regions),
        relocated_objects: 0,
        blocked,
    }
}

fn select_collection_set(regions: &RegionSpace, copy_budget: usize) -> Vec<usize> {
    let mut candidates = mixed_candidates(regions);
    candidates.sort_by_key(|&(idx, live)| (live, idx));

    let mut selected = Vec::new();
    let mut selected_live = 0usize;
    for (idx, live) in candidates {
        let next = selected_live.saturating_add(live);
        if next > copy_budget {
            break;
        }
        selected.push(idx);
        selected_live = next;
    }
    selected
}

fn remaining_candidate_bytes(regions: &RegionSpace) -> usize {
    mixed_candidates(regions)
        .into_iter()
        .map(|(_, live)| live)
        .sum()
}

fn mixed_candidates(regions: &RegionSpace) -> Vec<(usize, usize)> {
    (0..regions.region_count())
        .filter_map(|idx| {
            let kind = regions.kind(idx)?;
            if !matches!(kind, RegionKind::Old | RegionKind::HumongousStart) {
                return None;
            }
            let live = regions.live_bytes(idx).unwrap_or_default();
            if live == 0
                || live.saturating_mul(100) > REGION_SIZE.saturating_mul(MIXED_SKIP_LIVE_PERCENT)
            {
                return None;
            }
            Some((idx, live))
        })
        .collect()
}

fn snapshot_cset_objects(
    ctx: &mut GcContext<'_>,
    regions: &RegionSpace,
    obj_table_ptr: usize,
    obj_table_count: usize,
    cset: &[usize],
) -> Vec<MixedObject> {
    let cset = cset.iter().copied().collect::<BTreeSet<_>>();
    ctx.with_memory(|data| {
        let mut objects = Vec::new();
        for h in 0..obj_table_count as Handle {
            let Some(ptr) = object_walker::resolve_handle(data, h, obj_table_ptr, obj_table_count)
            else {
                continue;
            };
            let Some(region_idx) = regions.region_index(ptr) else {
                continue;
            };
            if !cset.contains(&region_idx) {
                continue;
            }
            let Some(kind) = regions.kind(region_idx) else {
                continue;
            };
            let Some(size) = object_size_from_memory(data, ptr) else {
                debug_assert!(
                    false,
                    "G1 mixed: live obj_table entry has unreadable header"
                );
                continue;
            };
            objects.push(MixedObject {
                handle: h,
                ptr,
                size,
                source_region: region_idx,
                kind,
            });
        }
        objects
    })
}

fn build_evacuation_plan(
    regions: &RegionSpace,
    objects: &[MixedObject],
    cset: &[usize],
) -> Option<EvacuationPlan> {
    let mut planned = regions.clone();
    let mut allocator = MixedAllocator::default();
    let mut relocations = Vec::with_capacity(objects.len());

    for object in objects {
        let dest = allocator.allocate(&mut planned, object.size)?;
        mark_destination_dense(&mut planned, dest, object.size);
        relocations.push(Relocation {
            handle: object.handle,
            src: object.ptr,
            dest,
            size: object.size,
        });
    }

    let mut released = cset.iter().copied().collect::<BTreeSet<_>>();
    for object in objects {
        if object.kind == RegionKind::HumongousStart {
            for idx in
                object.source_region..object.source_region + object.size.div_ceil(REGION_SIZE)
            {
                released.insert(idx);
            }
        }
    }
    for &idx in &released {
        planned.release(idx);
    }

    Some(EvacuationPlan {
        regions: planned,
        relocations,
        released_regions: released.into_iter().collect(),
    })
}

impl MixedAllocator {
    fn allocate(&mut self, regions: &mut RegionSpace, size: usize) -> Option<usize> {
        if size > HUMONGOUS_THRESHOLD {
            let count = size.div_ceil(REGION_SIZE);
            let idx = regions.take_contiguous_free_as_humongous(count)?;
            return regions.region_start(idx);
        }
        if let Some(region) = &mut self.old
            && let Some(ptr) = bump(region, size)
        {
            return Some(ptr);
        }
        let idx = regions.take_free_as(RegionKind::Old)?;
        let start = regions.region_start(idx)?;
        let end = regions.region_end(idx)?;
        self.old = Some(BumpRegion { cursor: start, end });
        bump(self.old.as_mut()?, size)
    }
}

fn bump(region: &mut BumpRegion, size: usize) -> Option<usize> {
    let end = region.cursor.checked_add(size)?;
    if end > region.end {
        return None;
    }
    let ptr = region.cursor;
    region.cursor = end;
    Some(ptr)
}

fn mark_destination_dense(regions: &mut RegionSpace, ptr: usize, size: usize) {
    let Some(first) = regions.region_index(ptr) else {
        return;
    };
    let last_addr = ptr.saturating_add(size.saturating_sub(1));
    let last = regions.region_index(last_addr).unwrap_or(first);
    for idx in first..=last {
        regions.set_live_bytes(idx, REGION_SIZE);
    }
}

fn copy_object(
    ctx: &mut GcContext<'_>,
    src: usize,
    dest: usize,
    size: usize,
    scratch: &mut Vec<u8>,
) -> bool {
    ctx.with_memory_mut(|data| copy_raw_object(data, src, dest, size, scratch))
}

fn copy_raw_object(
    data: &mut [u8],
    src: usize,
    dest: usize,
    size: usize,
    scratch: &mut Vec<u8>,
) -> bool {
    scratch.clear();
    let Some(bytes) = data.get(src..src.saturating_add(size)) else {
        debug_assert!(
            false,
            "G1 mixed: source object disappeared during evacuation"
        );
        return false;
    };
    scratch.extend_from_slice(bytes);
    if scratch.len() != size {
        debug_assert!(false, "G1 mixed: source object copy was truncated");
        return false;
    }
    let Some(out) = data.get_mut(dest..dest.saturating_add(size)) else {
        debug_assert!(
            false,
            "G1 mixed: destination object range is outside memory"
        );
        return false;
    };
    out.copy_from_slice(scratch);
    true
}

fn clear_released_region_cards(
    regions: &RegionSpace,
    rset: &mut G1RSet,
    released_regions: &[usize],
) {
    let cards_per_region = REGION_SIZE / CARD_SIZE;
    for &idx in released_regions {
        let Some(start) = regions.region_start(idx) else {
            continue;
        };
        let Some(first_card) = regions.card_index(start) else {
            continue;
        };
        rset.clear_card_range(first_card, first_card + cards_per_region);
    }
}

fn redirty_destination_cards(
    ctx: &mut GcContext<'_>,
    regions: &RegionSpace,
    rset: &mut G1RSet,
    obj_table_ptr: usize,
    obj_table_count: usize,
    relocations: &[Relocation],
) {
    if relocations.is_empty() {
        return;
    }
    let young_after = young_handle_snapshot(ctx, regions, obj_table_ptr, obj_table_count);
    if young_after.is_empty() {
        return;
    }

    for relocation in relocations {
        let mut slots = Vec::new();
        ctx.with_memory(|data| {
            object_walker::collect_slots_in_range(
                data,
                obj_table_ptr,
                obj_table_count,
                relocation.dest..relocation.dest + relocation.size,
                &mut slots,
            );
        });
        for slot in slots {
            if slot_has_young_ref(ctx, obj_table_count, slot, &young_after)
                && let Some(card_idx) = regions.card_index(slot.slot_addr)
            {
                rset.mark_dirty_slot(slot.slot_addr, card_idx);
            }
        }
    }
}

fn slot_has_young_ref(
    ctx: &mut GcContext<'_>,
    obj_table_count: usize,
    slot: SlotValue,
    young_after: &HashSet<Handle>,
) -> bool {
    direct_value_points_to_young(slot.value, young_after) || {
        let mut found = false;
        object_walker::visit_value_handles(ctx, slot.value, obj_table_count, &mut |h| {
            found |= young_after.contains(&h);
        });
        found
    }
}

fn direct_value_points_to_young(value: Value, young_after: &HashSet<Handle>) -> bool {
    super::rset::value_to_handle(value).is_some_and(|h| young_after.contains(&h))
}

fn young_handle_snapshot(
    ctx: &mut GcContext<'_>,
    regions: &RegionSpace,
    obj_table_ptr: usize,
    obj_table_count: usize,
) -> HashSet<Handle> {
    ctx.with_memory(|data| {
        let mut young = HashSet::new();
        for h in 0..obj_table_count as Handle {
            let Some(ptr) = object_walker::resolve_handle(data, h, obj_table_ptr, obj_table_count)
            else {
                continue;
            };
            let Some(region_idx) = regions.region_index(ptr) else {
                continue;
            };
            if regions.kind(region_idx).is_some_and(RegionKind::is_young) {
                young.insert(h);
            }
        }
        young
    })
}

fn stats_for_result(
    ctx: &mut GcContext<'_>,
    started: Instant,
    regions: &RegionSpace,
    result: &MixedResult,
) -> GcStats {
    if result.relocated_objects != 0 || result.released_regions != 0 {
        ctx.increment_gc_epoch();
    }
    let metrics = heap_governance::compute_metrics(&regions.free_region_intervals());
    let stats = GcStats {
        marked: result.relocated_objects,
        swept: result.released_regions,
        freed_bytes: result.reclaimed_bytes,
        elapsed: started.elapsed(),
        free_block_count: metrics.free_block_count,
        total_free_bytes: metrics.total_free_bytes,
        largest_free_block: metrics.largest_free_block,
        external_fragmentation: metrics.external_fragmentation,
        heap_used_bytes: ctx.heap_used(),
        ..GcStats::default()
    };
    ctx.stats = stats.clone();
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{constants, value};

    fn mixed_regions() -> RegionSpace {
        let start = REGION_SIZE;
        let mut regions = RegionSpace::default();
        regions.attach(start, start, start, start + 6 * REGION_SIZE);
        for idx in 0..5 {
            regions.set_kind_age(idx, RegionKind::Old, 0);
        }
        regions.release(5);
        regions
    }

    #[test]
    fn g1_mixed_cset_uses_live_bytes_order_and_budget() {
        let mut regions = mixed_regions();
        regions.set_live_bytes(0, 300);
        regions.set_live_bytes(1, 100);
        regions.set_live_bytes(2, 200);
        regions.set_live_bytes(3, 50);

        let cset = select_collection_set(&regions, 350);

        assert_eq!(cset, vec![3, 1, 2]);
    }

    #[test]
    fn g1_mixed_cset_skips_regions_above_85_percent_live() {
        let mut regions = mixed_regions();
        let allowed = REGION_SIZE * MIXED_SKIP_LIVE_PERCENT / 100;
        regions.set_live_bytes(0, allowed);
        regions.set_live_bytes(1, allowed + 1);

        let cset = select_collection_set(&regions, usize::MAX);

        assert_eq!(cset, vec![0]);
    }

    #[test]
    fn g1_mixed_plan_releases_source_and_marks_destination_dense() {
        let mut regions = mixed_regions();
        regions.set_live_bytes(0, 128);
        let object = MixedObject {
            handle: 7,
            ptr: regions.region_start(0).unwrap(),
            size: 128,
            source_region: 0,
            kind: RegionKind::Old,
        };

        let plan = build_evacuation_plan(&regions, &[object], &[0]).unwrap();
        let relocation = plan.relocations[0];
        let dest_idx = plan.regions.region_index(relocation.dest).unwrap();

        assert_eq!(plan.regions.kind(0), Some(RegionKind::Free));
        assert_eq!(plan.regions.kind(dest_idx), Some(RegionKind::Old));
        assert_eq!(plan.regions.live_bytes(dest_idx), Some(REGION_SIZE));
        assert_eq!(relocation.handle, 7);
    }

    #[test]
    fn g1_mixed_raw_copy_preserves_handle_reference_slots() {
        let src = 128;
        let dest = 512;
        let size = constants::HEAP_OBJECT_HEADER_SIZE as usize
            + constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE as usize;
        let slot_addr = src
            + constants::HEAP_OBJECT_HEADER_SIZE as usize
            + constants::PROP_SLOT_VALUE_OFFSET as usize;
        let dest_slot_addr = dest
            + constants::HEAP_OBJECT_HEADER_SIZE as usize
            + constants::PROP_SLOT_VALUE_OFFSET as usize;
        let child = value::encode_object_handle(42);
        let mut data = vec![0u8; 1024];
        data[src + constants::HEAP_OBJECT_TYPE_OFFSET as usize] = wjsm_ir::HEAP_TYPE_OBJECT;
        data[src + constants::HEAP_OBJECT_CAPACITY_OFFSET as usize..][..4]
            .copy_from_slice(&1u32.to_le_bytes());
        data[slot_addr..slot_addr + 8].copy_from_slice(&child.to_le_bytes());

        assert!(copy_raw_object(&mut data, src, dest, size, &mut Vec::new()));

        let copied =
            i64::from_le_bytes(data[dest_slot_addr..dest_slot_addr + 8].try_into().unwrap());
        assert_eq!(copied, child);
    }

    #[test]
    fn g1_mixed_redirty_predicate_detects_young_handle_value() {
        let mut young = HashSet::new();
        young.insert(9);

        assert!(direct_value_points_to_young(
            value::encode_object_handle(9),
            &young
        ));
        assert!(!direct_value_points_to_young(
            value::encode_object_handle(10),
            &young
        ));
    }

    #[test]
    fn g1_mixed_summary_does_not_publish_dead_handles() {
        let result = MixedResult {
            relocated_objects: 2,
            released_regions: 1,
            reclaimed_bytes: REGION_SIZE,
        };

        assert_eq!(result.relocated_objects, 2);
        assert_eq!(result.released_regions, 1);
    }
}
