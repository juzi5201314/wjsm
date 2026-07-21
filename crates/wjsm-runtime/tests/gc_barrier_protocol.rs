#![cfg(feature = "managed-heap-v2")]

use std::sync::atomic::AtomicU64;

use wasmtime::{MemoryType, SharedMemory};
use wjsm_engine_config::EngineConfig;
use wjsm_ir::value::{
    self, encode_f64, encode_null, encode_object_handle, encode_runtime_string_handle,
    encode_undefined, strip_gc_color,
};
use wjsm_runtime::{
    BarrierEpoch, BarrierRecord, BarrierRing, BulkCopyMode, HANDLE_REGION_BYTES, HandleGeneration,
    HandleId, HandleTableV2, HeaderLayout, LoadBarrierOutcome, ManagedHeapLayout,
    color_stored_value, load_barrier, prototype_field_kind, select_bulk_copy_mode, store_barrier,
    store_barrier_with_target_generation,
};

const PAGE: u64 = 64 * 1024;

fn layout() -> ManagedHeapLayout {
    ManagedHeapLayout::new(PAGE * 8, PAGE).unwrap()
}

fn table() -> HandleTableV2 {
    HandleTableV2::new(layout()).unwrap()
}

fn publish(table: &HandleTableV2, generation: HandleGeneration) -> HandleId {
    let handle = table.allocate_handle().unwrap();
    let address = table.layout().object_heap_base() + u64::from(handle.get()) * 64;
    table.publish(handle, address, generation).unwrap();
    handle
}

#[test]
fn stable_load_barrier_returns_address_without_assist() {
    let table = table();
    let handle = publish(&table, HandleGeneration::Young);
    match load_barrier(&table, handle) {
        LoadBarrierOutcome::Stable {
            address,
            generation,
        } => {
            assert_eq!(generation, HandleGeneration::Young);
            assert_eq!(address, table.resolve(handle).unwrap().address());
        }
        other => panic!("expected stable load, got {other:?}"),
    }
}

#[test]
fn relocating_load_barrier_requires_assist() {
    let table = table();
    let handle = publish(&table, HandleGeneration::Young);
    table.begin_relocation(handle).unwrap();
    match load_barrier(&table, handle) {
        LoadBarrierOutcome::Relocating { generation, .. } => {
            assert_eq!(generation, HandleGeneration::Young);
        }
        other => panic!("expected relocating load, got {other:?}"),
    }
}

#[test]
fn satb_old_to_young_and_one_slot_buffer() {
    let epoch = BarrierEpoch {
        young_marking: true,
        young_mark: 0b01,
        ..BarrierEpoch::IDLE
    };
    let old = encode_object_handle(11);
    let slot = AtomicU64::new(old as u64);
    let (_stored, records) = store_barrier_with_target_generation(
        epoch,
        HandleGeneration::Old,
        Some(HandleGeneration::Young),
        &slot,
        encode_object_handle(12),
        0xABCD,
    );
    assert!(records.contains(&BarrierRecord::Satb(HandleId::new(11))));
    assert!(records.contains(&BarrierRecord::RememberedSlot { slot_addr: 0xABCD }));

    let ring = BarrierRing::with_capacity(1);
    assert!(ring.try_push(HandleId::new(11)));
    assert_eq!(
        ring.push_or_mark_full(HandleId::new(12)),
        Err(HandleId::new(12))
    );
    assert_eq!(ring.host_flushes(), 1);
    assert_eq!(ring.drain(), vec![HandleId::new(11)]);
}

#[test]
fn runtime_string_reference_colored_non_references_clear() {
    let epoch = BarrierEpoch {
        young_marking: true,
        young_mark: 0b10,
        ..BarrierEpoch::IDLE
    };
    let runtime_string = encode_runtime_string_handle(5);
    let colored = color_stored_value(epoch, runtime_string);
    assert_ne!(colored as u64 & value::GC_COLOR_MASK, 0);
    assert_eq!(strip_gc_color(colored), runtime_string);

    for scalar in [
        encode_f64(1.0),
        encode_null(),
        encode_undefined(),
        value::encode_bool(true),
        value::encode_string_ptr(9),
    ] {
        let stored = color_stored_value(epoch, scalar);
        assert_eq!(
            stored as u64 & value::GC_COLOR_MASK,
            0,
            "scalar {scalar:#x}"
        );
    }
}

#[test]
fn mutable_prototype_header_and_bulk_copy_verifier() {
    assert_eq!(
        prototype_field_kind(),
        wjsm_runtime::HeaderFieldKind::MutableAtomicWord
    );
    assert!(HeaderLayout::OBJECT.rejects_bulk_copy_of_mutable_headers());
    assert_eq!(
        select_bulk_copy_mode(
            true,
            HandleGeneration::Young,
            HandleGeneration::Young,
            HeaderLayout::OBJECT
        ),
        BulkCopyMode::PerSlotBarrier
    );
    assert_eq!(
        select_bulk_copy_mode(
            false,
            HandleGeneration::Young,
            HandleGeneration::Old,
            HeaderLayout::OBJECT
        ),
        BulkCopyMode::PrePublish
    );
}

#[test]
fn store_barrier_seqcst_publishes_colored_word() {
    let epoch = BarrierEpoch {
        young_marking: true,
        young_mark: 0b01,
        ..BarrierEpoch::IDLE
    };
    let slot = AtomicU64::new(0);
    let (stored, _) = store_barrier(
        epoch,
        HandleGeneration::Young,
        &slot,
        encode_object_handle(3),
        HANDLE_REGION_BYTES + 64,
    );
    assert_eq!(
        slot.load(std::sync::atomic::Ordering::SeqCst) as i64,
        stored
    );
    assert_ne!(stored as u64 & value::GC_COLOR_MASK, 0);
}

#[test]
fn load_barrier_missing_handle_is_invalid() {
    let table = table();
    assert_eq!(
        load_barrier(&table, HandleId::new(0xFFFF)),
        LoadBarrierOutcome::Invalid
    );
}

#[test]
fn shared_memory_word_store_is_seqcst_visible() {
    let engine = EngineConfig::artifact().build().unwrap();
    let memory = SharedMemory::new(
        &engine,
        MemoryType::builder()
            .memory64(true)
            .shared(true)
            .min(2)
            .max(Some(4))
            .build()
            .unwrap(),
    )
    .unwrap();
    let heap = wjsm_runtime::SharedHeapMemory::new(memory);
    heap.grow_to(16).unwrap();
    let addr = wjsm_runtime::HeapAddress::new(8);
    heap.store_word(addr, 0x1111).unwrap();
    assert_eq!(heap.load_word(addr).unwrap(), 0x1111);
}
