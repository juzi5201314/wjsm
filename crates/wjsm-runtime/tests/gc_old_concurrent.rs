use std::time::Duration;

use wjsm_runtime::{
    GcRuntimeV2, HandleGeneration, HandleId, OldController, OldPhase, YoungController,
};

fn roots(handles: impl IntoIterator<Item = HandleId>) -> wjsm_runtime::RootSnapshot {
    let runtime = GcRuntimeV2::new();
    let mutator = runtime.register_mutator();
    runtime.request_root_snapshot();
    mutator.publish_roots(handles.into_iter().map(HandleId::get))
}

#[test]
fn old_mark_starts_from_young_mark_start_and_spans_cycles() {
    let young = YoungController::new(4);
    let old = OldController::new();
    let old_a = HandleId::new(1);
    let old_b = HandleId::new(2);
    let young_root = HandleId::new(3);
    old.register_object(old_a, HandleGeneration::Old, [Some(old_b)], 128);
    old.register_object(old_b, HandleGeneration::Old, [], 64);
    old.register_object(young_root, HandleGeneration::Young, [Some(old_a)], 32);
    young.register_object(
        young_root,
        HandleGeneration::Young,
        [Some(old_a)],
        false,
        false,
    );

    let r1 = roots([young_root, old_a]);
    old.coordinate_from_young_mark_start(&young, &r1, true);
    assert!(old.is_active());
    assert_eq!(old.phase(), OldPhase::ConcurrentMark);
    assert!(old.concurrent_mark_step(1));

    // second young cycle while old still marking
    let r2 = roots([young_root]);
    old.coordinate_from_young_mark_start(&young, &r2, true);
    assert_eq!(old.report().young_cycles_spanned, 1);
    assert!(old.report().young_to_old_roots >= 1);

    // promotion frontier from young
    let promoted = HandleId::new(9);
    old.register_object(promoted, HandleGeneration::Young, [], 40);
    old.note_promoted(promoted);
    old.coordinate_from_young_mark_start(&young, &roots([young_root]), true);
    assert!(old.report().promotion_frontier >= 1);

    while old.concurrent_mark_step(8) {}
    let pause = old.pause_mark_end();
    assert!(pause < Duration::from_millis(1));
    assert!(old.is_marked(old_a));
    assert!(old.is_marked(old_b));
    assert!(old.mark_work_normalized_by_old_live().unwrap() <= 1.0 + f64::EPSILON);
}

#[test]
fn young_pause_does_not_run_full_old_work() {
    let young = YoungController::new(4);
    let old = OldController::new();
    for i in 0..1_000u32 {
        old.register_object(HandleId::new(i), HandleGeneration::Old, [], 64);
    }
    let root = HandleId::new(0);
    old.coordinate_from_young_mark_start(&young, &roots([root]), true);
    // young pause mark end path only allows tiny residual old work
    let pause = old.pause_mark_end();
    assert!(pause < Duration::from_millis(1));
    // still concurrent work remaining for large old heap
    assert!(old.is_active());
    assert!(old.report().marked < 1_000);
}
