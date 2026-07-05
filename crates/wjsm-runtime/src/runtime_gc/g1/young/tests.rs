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

#[test]
fn g1_young_preserves_new_survivor_regions_when_releasing_sources() {
    let object_heap_start = REGION_SIZE;
    let mut regions = RegionSpace::default();
    regions.attach(
        object_heap_start,
        object_heap_start,
        object_heap_start,
        object_heap_start + 4 * REGION_SIZE,
    );
    let mut allocator = EvacuationAllocator::new();
    let dest = allocator.allocate_survivor(&mut regions, 128, 1).unwrap();
    let dest_idx = regions.region_index(dest).unwrap();
    let mut retained = HashSet::new();

    retained.extend(allocator.survivor_region_indices());
    for idx in regions.young_region_indices() {
        if !retained.contains(&idx) {
            regions.release(idx);
        }
    }

    assert_eq!(regions.kind(dest_idx), Some(RegionKind::Survivor));
}
