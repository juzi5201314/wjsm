use wjsm_runtime::{GcAlgorithmKind, RuntimeOptions};

#[test]
fn runtime_options_default() {
    let options = RuntimeOptions::default();
    assert_eq!(options.gc_algorithm, GcAlgorithmKind::MarkSweep);
    assert_eq!(options.max_heap_size, None);
    assert_eq!(options.inspect, None);
}
