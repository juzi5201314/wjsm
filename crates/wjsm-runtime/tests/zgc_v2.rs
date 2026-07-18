#![cfg(feature = "managed-heap-v2")]

use wasmtime::{MemoryType, SharedMemory};
use wjsm_engine_config::EngineConfig;
use wjsm_runtime::{
    GcRuntimeV2, HandleId, ManagedHeapLayout, RootSnapshot, SharedHeapMemory, ZgcV2, ZgcV2Error,
    ZgcV2Phase, ZgcV2StepOutcome,
};

const HANDLE_REGION_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const PAGE_BYTES: u64 = 64 * 1024;

fn shared_heap() -> SharedHeapMemory {
    let engine = EngineConfig::artifact().build().unwrap();
    let memory = SharedMemory::new(
        &engine,
        MemoryType::builder()
            .memory64(true)
            .shared(true)
            .min(HANDLE_REGION_BYTES / PAGE_BYTES + 1)
            .max(Some(HANDLE_REGION_BYTES / PAGE_BYTES + 64))
            .build()
            .unwrap(),
    )
    .unwrap();
    SharedHeapMemory::new(memory)
}

fn root_snapshot(handles: impl IntoIterator<Item = HandleId>) -> RootSnapshot {
    let runtime = GcRuntimeV2::new();
    let mutator = runtime.register_mutator();
    runtime.request_root_snapshot();
    mutator.publish_roots(handles.into_iter().map(HandleId::get))
}

#[test]
fn zgc_v2_incremental_mark_relocate_preserves_handle_identity() {
    let layout = ManagedHeapLayout::new(PAGE_BYTES * 8, PAGE_BYTES).unwrap();
    let collector = ZgcV2::new(shared_heap(), layout).unwrap();
    let dead = collector.allocate(1024, []).unwrap();
    let child = collector.allocate(1024, []).unwrap();
    let root = collector.allocate(1024, [child]).unwrap();
    collector.write_payload(child, &[42]).unwrap();
    let child_address = collector.address(child).unwrap();
    let root_address = collector.address(root).unwrap();
    let roots = root_snapshot([root]);
    let mut cleaned = Vec::new();
    let mut seen_phases = Vec::new();

    let report = (0..8)
        .find_map(
            |_| match collector.safepoint_step(&roots, 1024, |handle| cleaned.push(handle)) {
                Ok(ZgcV2StepOutcome::Progress { phase, .. }) => {
                    seen_phases.push(phase);
                    None
                }
                Ok(ZgcV2StepOutcome::CycleComplete(report)) => Some(report),
                Err(error) => panic!("incremental ZGC V2 step failed: {error}"),
            },
        )
        .expect("incremental ZGC V2 cycle did not complete");

    assert_eq!(collector.phase(), ZgcV2Phase::Idle);
    assert!(seen_phases.contains(&ZgcV2Phase::Mark));
    assert!(seen_phases.contains(&ZgcV2Phase::Relocate));
    assert_eq!(report.marked, 2);
    assert_eq!(report.retired, 1);
    assert_eq!(report.relocated, 2);
    assert_eq!(cleaned, vec![dead]);
    assert!(collector.is_marked(root).unwrap());
    assert!(collector.is_marked(child).unwrap());
    assert_ne!(collector.address(root).unwrap(), root_address);
    assert_ne!(collector.address(child).unwrap(), child_address);
    assert_eq!(collector.read_payload(child, 1).unwrap(), vec![42]);
    assert_eq!(collector.telemetry_snapshot().cycles, 1);
}

#[test]
fn zgc_v2_rejects_reference_mutation_during_relocation() {
    let layout = ManagedHeapLayout::new(PAGE_BYTES * 4, PAGE_BYTES).unwrap();
    let collector = ZgcV2::new(shared_heap(), layout).unwrap();
    let root = collector.allocate(1024, []).unwrap();
    let roots = root_snapshot([root]);

    let step = collector.safepoint_step(&roots, 1024, |_| {}).unwrap();

    assert_eq!(collector.phase(), ZgcV2Phase::Relocate);
    assert!(matches!(step, ZgcV2StepOutcome::Progress { .. }));
    assert!(matches!(
        collector.write_reference(root, 0, None),
        Err(ZgcV2Error::RelocationInProgress)
    ));
    assert!(matches!(
        collector.safepoint_step(&roots, 1024, |_| {}),
        Ok(ZgcV2StepOutcome::CycleComplete(report)) if report.relocated == 1
    ));
}
