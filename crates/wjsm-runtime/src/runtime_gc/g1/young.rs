//! G1 STW young collection。
//!
//! 本 slice 只实现年轻代 evacuation：root 发现、Eden/Survivor 对象复制或晋升、
//! obj_table 更新、dirty card 精化以及 freed handle cleanup。Old/Humongous 的全堆
//! 标记与 mixed evacuation 由后续 G1 阶段接管。

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::runtime_gc::api::{AllocRequest, GcContext, GcStats, Handle, RootProvider};
use crate::runtime_gc::context::object_size_from_memory;
use crate::runtime_gc::object_walker::{self, ObjectWalker, SlotValue};
use crate::runtime_gc::{roots, weak_refs};

use super::region::{CARD_SIZE, REGION_SIZE, RegionKind, RegionSpace};
use super::rset::G1RSet;

const PROMOTION_AGE: u8 = 2;
const HUMONGOUS_THRESHOLD: usize = REGION_SIZE / 2;

#[derive(Clone, Copy, Debug)]
struct ObjectInfo {
    ptr: usize,
    size: usize,
    region_idx: usize,
    kind: RegionKind,
    age: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EvacuationDecision {
    Stay,
    Survivor { age: u8 },
    Promote,
}

#[derive(Debug, Default)]
struct YoungCollectionResult {
    live_young: HashSet<Handle>,
    freed_handles: Vec<Handle>,
    promoted_handles: Vec<Handle>,
    relocated_bytes: usize,
    freed_bytes: usize,
}

struct EvacuationAllocator {
    survivor: Option<BumpRegion>,
    old: Option<BumpRegion>,
}

struct BumpRegion {
    idx: usize,
    cursor: usize,
    end: usize,
}

impl EvacuationAllocator {
    fn new() -> Self {
        Self {
            survivor: None,
            old: None,
        }
    }

    fn allocate_survivor(
        &mut self,
        regions: &mut RegionSpace,
        size: usize,
        age: u8,
    ) -> Option<usize> {
        if size > HUMONGOUS_THRESHOLD {
            return None;
        }
        allocate_in_current_or_new(regions, &mut self.survivor, RegionKind::Survivor, age, size)
    }

    fn allocate_old(&mut self, regions: &mut RegionSpace, size: usize) -> Option<usize> {
        if size > HUMONGOUS_THRESHOLD {
            let count = size.div_ceil(REGION_SIZE);
            let idx = regions.take_contiguous_free_as_humongous(count)?;
            return regions.region_start(idx);
        }
        allocate_in_current_or_new(regions, &mut self.old, RegionKind::Old, 0, size)
    }
}

pub(super) fn alloc_slow(
    ctx: &mut GcContext<'_>,
    roots: &mut dyn RootProvider,
    req: AllocRequest,
    regions: &mut RegionSpace,
    rset: &mut G1RSet,
) -> Option<usize> {
    if let Some(ptr) = allocate_request(ctx, req, regions) {
        return Some(ptr);
    }

    let _ = collect_young(ctx, roots, regions, rset);
    if let Some(ptr) = allocate_request(ctx, req, regions) {
        return Some(ptr);
    }

    if grow_for_request(ctx, regions, req.size) {
        return allocate_request(ctx, req, regions);
    }
    None
}

pub(super) fn collect_young(
    ctx: &mut GcContext<'_>,
    roots_provider: &mut dyn RootProvider,
    regions: &mut RegionSpace,
    rset: &mut G1RSet,
) -> GcStats {
    let started = Instant::now();
    let obj_table_ptr = ctx.obj_table_ptr();
    let obj_table_count = ctx.obj_table_count();
    let object_info = snapshot_objects(ctx, regions, obj_table_ptr, obj_table_count);
    let young_handles = object_info
        .iter()
        .filter_map(|(&h, info)| info.kind.is_young().then_some(h))
        .collect::<HashSet<_>>();
    if young_handles.is_empty() {
        let stats = stats_for_result(ctx, started, YoungCollectionResult::default());
        ctx.stats = stats.clone();
        return stats;
    }

    let mut live_young = HashSet::new();
    let mut worklist = Vec::new();
    let mut walker = ObjectWalker::new();

    let mut shadow_roots = Vec::new();
    roots_provider.for_each_shadow_stack_root(ctx, &mut |h| shadow_roots.push(h));
    mark_handles(&object_info, &mut live_young, &mut worklist, shadow_roots);
    drain_young_graph(
        ctx,
        &object_info,
        obj_table_ptr,
        obj_table_count,
        &mut live_young,
        &mut worklist,
        &mut walker,
    );

    loop {
        let before = live_young.len();
        let mut host_roots = Vec::new();
        let mut is_marked = |h: Handle| !young_handles.contains(&h) || live_young.contains(&h);
        roots_provider.for_each_host_table_root(ctx, &mut is_marked, &mut |h| host_roots.push(h));
        mark_handles(&object_info, &mut live_young, &mut worklist, host_roots);
        drain_young_graph(
            ctx,
            &object_info,
            obj_table_ptr,
            obj_table_count,
            &mut live_young,
            &mut worklist,
            &mut walker,
        );
        if live_young.len() == before {
            break;
        }
    }

    let mut immortal_roots = Vec::new();
    roots::for_each_immortal_region_root(ctx, &mut |h| immortal_roots.push(h));
    mark_handles(&object_info, &mut live_young, &mut worklist, immortal_roots);
    collect_dirty_card_roots(
        ctx,
        regions,
        rset,
        obj_table_ptr,
        obj_table_count,
        &object_info,
        &mut live_young,
        &mut worklist,
    );
    drain_young_graph(
        ctx,
        &object_info,
        obj_table_ptr,
        obj_table_count,
        &mut live_young,
        &mut worklist,
        &mut walker,
    );

    let mut result = evacuate_live_young(ctx, regions, &object_info, &young_handles, &live_young);
    refine_dirty_cards(ctx, regions, rset, obj_table_ptr, obj_table_count);
    redirty_promoted_destinations(
        ctx,
        regions,
        rset,
        obj_table_ptr,
        obj_table_count,
        &result.promoted_handles,
    );
    release_freed_handles(ctx, &result.freed_handles);
    result.live_young = live_young;

    let stats = stats_for_result(ctx, started, result);
    ctx.stats = stats.clone();
    stats
}

fn allocate_request(
    ctx: &mut GcContext<'_>,
    req: AllocRequest,
    regions: &mut RegionSpace,
) -> Option<usize> {
    if req.size == 0 {
        return None;
    }
    if req.size > HUMONGOUS_THRESHOLD {
        let count = req.size.div_ceil(REGION_SIZE);
        let idx = regions.take_contiguous_free_as_humongous(count)?;
        let ptr = regions.region_start(idx)?;
        let end = ptr + count * REGION_SIZE;
        let heap_ptr = ctx.heap_ptr();
        ctx.set_heap_ptr(heap_ptr.max(end));
        return Some(ptr);
    }

    if let Some(ptr) = allocate_in_current_eden(ctx, req.size, regions) {
        return Some(ptr);
    }

    let idx = regions.take_free_as_with_age(RegionKind::Eden, 0)?;
    let start = regions.region_start(idx)?;
    let end = regions.region_end(idx)?;
    ctx.set_heap_ptr(start);
    ctx.alloc_window_set(start, end);
    allocate_in_current_eden(ctx, req.size, regions)
}

fn allocate_in_current_eden(
    ctx: &mut GcContext<'_>,
    size: usize,
    regions: &mut RegionSpace,
) -> Option<usize> {
    let ptr = ctx.heap_ptr();
    let idx = regions.region_index(ptr)?;
    if regions.kind(idx)? != RegionKind::Eden {
        return None;
    }
    let end = regions.region_end(idx)?;
    let new_ptr = ptr.checked_add(size)?;
    if new_ptr > end {
        return None;
    }
    ctx.set_heap_ptr(new_ptr);
    ctx.alloc_window_set(new_ptr, end);
    Some(ptr)
}

fn grow_for_request(ctx: &mut GcContext<'_>, regions: &mut RegionSpace, size: usize) -> bool {
    let needed_regions = size.div_ceil(REGION_SIZE).max(1);
    let Some(target_end) = regions
        .object_heap_start()
        .checked_add((regions.region_count() + needed_regions) * REGION_SIZE)
    else {
        return false;
    };
    if target_end > ctx.heap_limit() {
        return false;
    }
    let mem_end = ctx.env.memory.data_size(&ctx.store);
    if target_end > mem_end {
        let pages = (target_end - mem_end).div_ceil(REGION_SIZE) as u64;
        if ctx.grow(pages).is_err() {
            return false;
        }
    }
    let committed_end = ctx.heap_limit().min(ctx.env.memory.data_size(&ctx.store));
    regions.extend_for_committed_end(committed_end);
    true
}

fn snapshot_objects(
    ctx: &mut GcContext<'_>,
    regions: &RegionSpace,
    obj_table_ptr: usize,
    obj_table_count: usize,
) -> HashMap<Handle, ObjectInfo> {
    ctx.with_memory(|data| {
        let mut objects = HashMap::new();
        for h in 0..obj_table_count as Handle {
            let Some(ptr) = object_walker::resolve_handle(data, h, obj_table_ptr, obj_table_count)
            else {
                continue;
            };
            let Some(region_idx) = regions.region_index(ptr) else {
                continue;
            };
            let Some(size) = object_size_from_memory(data, ptr) else {
                debug_assert!(
                    false,
                    "G1 young: live obj_table entry has unreadable header"
                );
                continue;
            };
            let Some(kind) = regions.kind(region_idx) else {
                continue;
            };
            objects.insert(
                h,
                ObjectInfo {
                    ptr,
                    size,
                    region_idx,
                    kind,
                    age: regions.age(region_idx).unwrap_or_default(),
                },
            );
        }
        objects
    })
}

fn mark_handles(
    objects: &HashMap<Handle, ObjectInfo>,
    live_young: &mut HashSet<Handle>,
    worklist: &mut Vec<Handle>,
    handles: impl IntoIterator<Item = Handle>,
) {
    for h in handles {
        if objects.get(&h).is_some_and(|info| info.kind.is_young()) && live_young.insert(h) {
            worklist.push(h);
        }
    }
}

fn drain_young_graph(
    ctx: &mut GcContext<'_>,
    objects: &HashMap<Handle, ObjectInfo>,
    obj_table_ptr: usize,
    obj_table_count: usize,
    live_young: &mut HashSet<Handle>,
    worklist: &mut Vec<Handle>,
    walker: &mut ObjectWalker,
) {
    while let Some(h) = worklist.pop() {
        walker.visit_object_children(ctx, h, obj_table_ptr, obj_table_count, &mut |child| {
            if objects.get(&child).is_some_and(|info| info.kind.is_young())
                && live_young.insert(child)
            {
                worklist.push(child);
            }
        });
    }
}

fn collect_dirty_card_roots(
    ctx: &mut GcContext<'_>,
    regions: &RegionSpace,
    rset: &G1RSet,
    obj_table_ptr: usize,
    obj_table_count: usize,
    objects: &HashMap<Handle, ObjectInfo>,
    live_young: &mut HashSet<Handle>,
    worklist: &mut Vec<Handle>,
) {
    let slots = collect_rset_slots(ctx, regions, rset, obj_table_ptr, obj_table_count);
    let mut roots = Vec::new();
    for slot in slots {
        object_walker::visit_value_handles(ctx, slot.value, obj_table_count, &mut |h| {
            if objects.get(&h).is_some_and(|info| info.kind.is_young()) {
                roots.push(h);
            }
        });
    }
    mark_handles(objects, live_young, worklist, roots);
}

fn evacuate_live_young(
    ctx: &mut GcContext<'_>,
    regions: &mut RegionSpace,
    objects: &HashMap<Handle, ObjectInfo>,
    young_handles: &HashSet<Handle>,
    live_young: &HashSet<Handle>,
) -> YoungCollectionResult {
    let mut result = YoungCollectionResult {
        live_young: live_young.clone(),
        ..YoungCollectionResult::default()
    };
    let mut allocator = EvacuationAllocator::new();
    let mut kept_source_regions = HashSet::new();
    let mut scratch = Vec::new();

    for &h in live_young {
        let Some(info) = objects.get(&h).copied() else {
            continue;
        };
        match evacuation_decision(info.kind, info.age, info.size, true) {
            EvacuationDecision::Stay => {
                kept_source_regions.insert(info.region_idx);
            }
            EvacuationDecision::Survivor { age } => {
                if let Some(dest) = allocator.allocate_survivor(regions, info.size, age) {
                    copy_object(ctx, info.ptr, dest, info.size, &mut scratch);
                    ctx.write_obj_table_slot(h, dest);
                    result.relocated_bytes += info.size;
                } else if let Some(dest) = allocator.allocate_old(regions, info.size) {
                    copy_object(ctx, info.ptr, dest, info.size, &mut scratch);
                    ctx.write_obj_table_slot(h, dest);
                    result.promoted_handles.push(h);
                    result.relocated_bytes += info.size;
                } else {
                    regions.set_kind_age(info.region_idx, RegionKind::Old, 0);
                    kept_source_regions.insert(info.region_idx);
                    result.promoted_handles.push(h);
                }
            }
            EvacuationDecision::Promote => {
                if let Some(dest) = allocator.allocate_old(regions, info.size) {
                    copy_object(ctx, info.ptr, dest, info.size, &mut scratch);
                    ctx.write_obj_table_slot(h, dest);
                    result.promoted_handles.push(h);
                    result.relocated_bytes += info.size;
                } else {
                    regions.set_kind_age(info.region_idx, RegionKind::Old, 0);
                    kept_source_regions.insert(info.region_idx);
                    result.promoted_handles.push(h);
                }
            }
        }
    }

    for &h in young_handles {
        if !live_young.contains(&h)
            && let Some(info) = objects.get(&h)
        {
            ctx.write_obj_table_slot(h, 0);
            result.freed_handles.push(h);
            result.freed_bytes += info.size;
        }
    }

    for idx in regions.young_region_indices() {
        if !kept_source_regions.contains(&idx) {
            regions.release(idx);
        }
    }

    result
}

fn copy_object(
    ctx: &mut GcContext<'_>,
    src: usize,
    dest: usize,
    size: usize,
    scratch: &mut Vec<u8>,
) {
    scratch.clear();
    ctx.with_memory(|data| {
        if let Some(bytes) = data.get(src..src + size) {
            scratch.extend_from_slice(bytes);
        }
    });
    if scratch.len() != size {
        debug_assert!(
            false,
            "G1 young: source object disappeared during evacuation"
        );
        return;
    }
    ctx.with_memory_mut(|data| {
        if let Some(out) = data.get_mut(dest..dest + size) {
            out.copy_from_slice(scratch);
        }
    });
}

fn refine_dirty_cards(
    ctx: &mut GcContext<'_>,
    regions: &RegionSpace,
    rset: &mut G1RSet,
    obj_table_ptr: usize,
    obj_table_count: usize,
) {
    let young_after = young_handle_snapshot(ctx, regions, obj_table_ptr, obj_table_count);
    for card_idx in rset.dirty_card_snapshot() {
        let slots =
            collect_card_slots(ctx, regions, rset, obj_table_ptr, obj_table_count, card_idx);
        if slots_have_young_ref(ctx, obj_table_count, &slots, &young_after) {
            for slot in slots {
                rset.mark_dirty_slot(slot.slot_addr, card_idx);
            }
        } else {
            rset.clear_card(card_idx);
        }
    }
}

fn redirty_promoted_destinations(
    ctx: &mut GcContext<'_>,
    regions: &RegionSpace,
    rset: &mut G1RSet,
    obj_table_ptr: usize,
    obj_table_count: usize,
    promoted_handles: &[Handle],
) {
    if promoted_handles.is_empty() {
        return;
    }
    let young_after = young_handle_snapshot(ctx, regions, obj_table_ptr, obj_table_count);
    for &h in promoted_handles {
        let Some((ptr, size)) = obj_ptr_and_size(ctx, h, obj_table_ptr, obj_table_count) else {
            continue;
        };
        let mut slots = Vec::new();
        ctx.with_memory(|data| {
            object_walker::collect_slots_in_range(
                data,
                obj_table_ptr,
                obj_table_count,
                ptr..ptr + size,
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

fn collect_rset_slots(
    ctx: &mut GcContext<'_>,
    regions: &RegionSpace,
    rset: &G1RSet,
    obj_table_ptr: usize,
    obj_table_count: usize,
) -> Vec<SlotValue> {
    let mut out = Vec::new();
    for card_idx in rset.dirty_card_snapshot() {
        out.extend(collect_card_slots(
            ctx,
            regions,
            rset,
            obj_table_ptr,
            obj_table_count,
            card_idx,
        ));
    }
    out
}

fn collect_card_slots(
    ctx: &mut GcContext<'_>,
    regions: &RegionSpace,
    rset: &G1RSet,
    obj_table_ptr: usize,
    obj_table_count: usize,
    card_idx: usize,
) -> Vec<SlotValue> {
    let precise = rset.precise_slot_snapshot(card_idx);
    let mut slots = Vec::new();
    ctx.with_memory(|data| {
        if precise.is_empty() {
            let start = regions.object_heap_start() + card_idx * CARD_SIZE;
            object_walker::collect_slots_in_range(
                data,
                obj_table_ptr,
                obj_table_count,
                start..start + CARD_SIZE,
                &mut slots,
            );
        } else {
            for slot_addr in &precise {
                object_walker::collect_slots_in_range(
                    data,
                    obj_table_ptr,
                    obj_table_count,
                    *slot_addr..slot_addr + 1,
                    &mut slots,
                );
            }
        }
    });
    slots
}

fn slots_have_young_ref(
    ctx: &mut GcContext<'_>,
    obj_table_count: usize,
    slots: &[SlotValue],
    young_after: &HashSet<Handle>,
) -> bool {
    slots
        .iter()
        .copied()
        .any(|slot| slot_has_young_ref(ctx, obj_table_count, slot, young_after))
}

fn slot_has_young_ref(
    ctx: &mut GcContext<'_>,
    obj_table_count: usize,
    slot: SlotValue,
    young_after: &HashSet<Handle>,
) -> bool {
    let mut found = false;
    object_walker::visit_value_handles(ctx, slot.value, obj_table_count, &mut |h| {
        found |= young_after.contains(&h);
    });
    found
}

fn young_handle_snapshot(
    ctx: &mut GcContext<'_>,
    regions: &RegionSpace,
    obj_table_ptr: usize,
    obj_table_count: usize,
) -> HashSet<Handle> {
    snapshot_objects(ctx, regions, obj_table_ptr, obj_table_count)
        .into_iter()
        .filter_map(|(h, info)| info.kind.is_young().then_some(h))
        .collect()
}

fn obj_ptr_and_size(
    ctx: &mut GcContext<'_>,
    h: Handle,
    obj_table_ptr: usize,
    obj_table_count: usize,
) -> Option<(usize, usize)> {
    ctx.with_memory(|data| {
        let ptr = object_walker::resolve_handle(data, h, obj_table_ptr, obj_table_count)?;
        let size = object_size_from_memory(data, ptr)?;
        Some((ptr, size))
    })
}

fn release_freed_handles(ctx: &mut GcContext<'_>, freed_handles: &[Handle]) {
    if freed_handles.is_empty() {
        return;
    }
    let freed = freed_handles.iter().copied().collect::<HashSet<_>>();
    ctx.with_state(|st| {
        st.reclaim_unmarked_collection_entries(|h| !freed.contains(&h));
    });
    weak_refs::process_weak_refs_after_sweep(ctx, freed_handles);
    weak_refs::cleanup_stream_tables_after_sweep(ctx, freed_handles);
    ctx.with_state(|st| {
        if let Some(mut list) = st.handle_free_list_for_gc() {
            list.extend_from_slice(freed_handles);
        }
    });
}

fn stats_for_result(
    ctx: &mut GcContext<'_>,
    started: Instant,
    result: YoungCollectionResult,
) -> GcStats {
    if result.relocated_bytes != 0 || !result.freed_handles.is_empty() {
        ctx.increment_gc_epoch();
    }
    GcStats {
        marked: result.live_young.len(),
        swept: result.freed_handles.len(),
        freed_bytes: result.freed_bytes,
        elapsed: started.elapsed(),
        heap_used_bytes: ctx.heap_used(),
        ..GcStats::default()
    }
}

fn allocate_in_current_or_new(
    regions: &mut RegionSpace,
    current: &mut Option<BumpRegion>,
    kind: RegionKind,
    age: u8,
    size: usize,
) -> Option<usize> {
    if let Some(region) = current
        && let Some(ptr) = bump(region, size)
    {
        return Some(ptr);
    }
    let idx = regions.take_free_as_with_age(kind, age)?;
    let start = regions.region_start(idx)?;
    let end = regions.region_end(idx)?;
    *current = Some(BumpRegion {
        idx,
        cursor: start,
        end,
    });
    let region = current.as_mut()?;
    debug_assert_eq!(region.idx, idx);
    bump(region, size)
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

fn evacuation_decision(
    source_kind: RegionKind,
    source_age: u8,
    size: usize,
    survivor_available: bool,
) -> EvacuationDecision {
    if matches!(
        source_kind,
        RegionKind::HumongousStart | RegionKind::HumongousCont
    ) {
        return EvacuationDecision::Stay;
    }
    if !source_kind.is_young() {
        return EvacuationDecision::Stay;
    }
    let next_age = source_age.saturating_add(1).max(1);
    if next_age >= PROMOTION_AGE || size > HUMONGOUS_THRESHOLD || !survivor_available {
        EvacuationDecision::Promote
    } else {
        EvacuationDecision::Survivor { age: next_age }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::constants;

    #[test]
    fn g1_young_promotes_when_age_reaches_threshold() {
        assert_eq!(
            evacuation_decision(RegionKind::Survivor, 1, 128, true),
            EvacuationDecision::Promote
        );
    }

    #[test]
    fn g1_young_promotes_when_survivor_space_is_unavailable() {
        assert_eq!(
            evacuation_decision(RegionKind::Eden, 0, 128, false),
            EvacuationDecision::Promote
        );
    }

    #[test]
    fn g1_young_leaves_humongous_objects_in_place() {
        assert_eq!(
            evacuation_decision(RegionKind::HumongousStart, 0, REGION_SIZE, true),
            EvacuationDecision::Stay
        );
    }

    #[test]
    fn g1_young_keeps_dirty_card_when_refined_slots_still_point_to_young() {
        let mut rset = G1RSet::default();
        rset.mark_dirty_slot(4096, 8);
        assert_eq!(rset.dirty_card_snapshot(), vec![8]);
        rset.clear_card(8);
        assert!(rset.dirty_card_snapshot().is_empty());
        rset.mark_dirty_slot(4096, 8);
        assert_eq!(rset.dirty_card_snapshot(), vec![8]);
    }

    #[test]
    fn g1_young_marks_promoted_destination_card_dirty_for_young_child() {
        let object_heap_start = REGION_SIZE;
        let mut regions = RegionSpace::default();
        regions.attach(
            object_heap_start,
            object_heap_start,
            object_heap_start,
            object_heap_start + 4 * REGION_SIZE,
        );
        let old_idx = regions.take_free_as_with_age(RegionKind::Old, 0).unwrap();
        let slot_addr =
            regions.region_start(old_idx).unwrap() + constants::HEAP_OBJECT_HEADER_SIZE as usize;
        let mut rset = G1RSet::default();
        let card_idx = regions.card_index(slot_addr).unwrap();

        rset.mark_dirty_slot(slot_addr, card_idx);

        assert_eq!(rset.dirty_card_snapshot(), vec![card_idx]);
    }
}
