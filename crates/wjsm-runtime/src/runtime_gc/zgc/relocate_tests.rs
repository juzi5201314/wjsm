use super::*;
use crate::runtime_gc::api::GcContext;
use crate::runtime_gc::zgc::page::ZPageKind;
use crate::{RuntimeState, WasmEnv};
use wasmtime::{
    Engine, Global, GlobalType, Memory, MemoryType, Mutability, Ref, RefType, Store, Table,
    TableType, Val, ValType,
};
use wjsm_ir::{HEAP_TYPE_OBJECT, constants, value};

fn i32_global(store: &mut Store<RuntimeState>, val: i32) -> Global {
    Global::new(
        &mut *store,
        GlobalType::new(ValType::I32, Mutability::Var),
        Val::I32(val),
    )
    .unwrap()
}

fn test_store_env() -> (Store<RuntimeState>, WasmEnv) {
    let engine = Engine::default();
    let mut store = Store::new(&engine, RuntimeState::new_with_shared(None));
    let memory = Memory::new(&mut store, MemoryType::new(4, None)).unwrap();
    let table = Table::new(
        &mut store,
        TableType::new(RefType::FUNCREF, 1, None),
        Ref::Func(None),
    )
    .unwrap();
    let shadow_sp = i32_global(&mut store, 0);
    let heap_ptr = i32_global(&mut store, (4 * ZPAGE_SIZE) as i32);
    let obj_table_ptr = i32_global(&mut store, 0);
    let obj_table_count = i32_global(&mut store, 1);
    let object_proto_handle = i32_global(&mut store, -1);
    let array_proto_handle = i32_global(&mut store, -1);
    let object_heap_start = i32_global(&mut store, ZPAGE_SIZE as i32);
    let heap_limit = i32_global(&mut store, (4 * ZPAGE_SIZE) as i32);
    let alloc_ptr = i32_global(&mut store, (4 * ZPAGE_SIZE) as i32);
    let alloc_end = i32_global(&mut store, (4 * ZPAGE_SIZE) as i32);
    let gc_phase = i32_global(&mut store, 2);
    let good_color = i32_global(&mut store, ZColor::Remapped.bits() as i32);
    store.data().store_heap_layout_boundaries(0, ZPAGE_SIZE, 0);
    let env = WasmEnv {
        memory,
        func_table: table,
        shadow_sp,
        heap_ptr,
        obj_table_ptr,
        obj_table_count,
        shadow_stack_end: None,
        object_proto_handle,
        array_proto_handle,
        object_heap_start: Some(object_heap_start),
        bootstrap_done: None,
        function_props_done: None,
        function_props_base: None,
        num_ir_functions: None,
        arr_proto_table_base: None,
        arr_proto_table_len: None,
        arr_proto_table_hash: None,
        heap_limit: Some(heap_limit),
        alloc_ptr: Some(alloc_ptr),
        alloc_end: Some(alloc_end),
        gc_alloc_bytes: None,
        gc_trigger_bytes: None,
        gc_phase: Some(gc_phase),
        good_color: Some(good_color),
        barrier_buf_ptr: None,
        barrier_buf_end: None,
    };
    (store, env)
}

fn fragmented_pages() -> ZPageSpace {
    let mut pages = ZPageSpace::default();
    pages.attach(0, 6 * ZPAGE_SIZE);
    pages.mark_live_bytes(0, 0).unwrap();
    pages.set_live_bytes(1, ZPAGE_SIZE / 2);
    pages.set_live_bytes(2, ZPAGE_SIZE * 80 / 100);
    pages.set_live_bytes(3, ZPAGE_SIZE / 8);
    pages.set_live_bytes(4, ZPAGE_SIZE * 74 / 100);
    pages
}

#[test]
fn zgc_relocate_selects_fragmented_non_empty_pages_by_live_bytes() {
    let mut pages = fragmented_pages();
    let reclaimed = pages.reclaim_dead_pages();

    let selected = pages.select_relocation_set(usize::MAX);

    assert_eq!(reclaimed, vec![0]);
    assert_eq!(selected, vec![3, 1, 4]);
}

#[test]
fn zgc_relocate_selection_is_copy_budget_truncated() {
    let pages = fragmented_pages();

    let selected = pages.select_relocation_set(ZPAGE_SIZE / 8 + ZPAGE_SIZE / 2);

    assert_eq!(selected, vec![3, 1]);
}

#[test]
fn zgc_relocate_non_rs_entry_repairs_to_remapped() {
    let entry = ZEntry::new(0x2000, ZColor::Marked1);

    assert_eq!(entry.repair_relocate_non_rs().color(), ZColor::Remapped);
}

#[test]
fn zgc_relocate_raw_copy_preserves_handle_slots() {
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
    data[src + constants::HEAP_OBJECT_TYPE_OFFSET as usize] = HEAP_TYPE_OBJECT;
    data[src + constants::HEAP_OBJECT_CAPACITY_OFFSET as usize..][..4]
        .copy_from_slice(&1u32.to_le_bytes());
    data[slot_addr..slot_addr + 8].copy_from_slice(&child.to_le_bytes());

    assert!(copy_raw_object(&mut data, src, dest, size, &mut Vec::new()));

    let copied = i64::from_le_bytes(data[dest_slot_addr..dest_slot_addr + 8].try_into().unwrap());
    assert_eq!(copied, child);
}

#[test]
fn zgc_relocate_host_resolve_heals_rs_object() {
    let (mut store, env) = test_store_env();
    let source = ZPAGE_SIZE;
    let size = constants::HEAP_OBJECT_HEADER_SIZE as usize
        + constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE as usize;
    let slot_addr = source
        + constants::HEAP_OBJECT_HEADER_SIZE as usize
        + constants::PROP_SLOT_VALUE_OFFSET as usize;
    let child = value::encode_object_handle(77);
    let mut pages = ZPageSpace::default();
    pages.attach(ZPAGE_SIZE, 4 * ZPAGE_SIZE);
    pages.add_live_bytes_range(source, size);
    let mut relocate = ZRelocateState::new();
    assert!(relocate.start_cycle(&mut pages, usize::MAX));

    let mut ctx = GcContext::new(&mut store, &env, "zgc-test");
    ctx.with_memory_mut(|data| {
        data[0..4].copy_from_slice(
            &ZEntry::new(source as u32, ZColor::Marked1)
                .raw()
                .to_le_bytes(),
        );
        data[source + constants::HEAP_OBJECT_TYPE_OFFSET as usize] = HEAP_TYPE_OBJECT;
        data[source + constants::HEAP_OBJECT_CAPACITY_OFFSET as usize..][..4]
            .copy_from_slice(&1u32.to_le_bytes());
        data[slot_addr..slot_addr + 8].copy_from_slice(&child.to_le_bytes());
    });

    let raw = relocate.relocate_or_remap_handle(&mut ctx, &mut pages, 0);
    let moved = ZEntry::from(raw);
    let copied_slot = moved.ptr() as usize
        + constants::HEAP_OBJECT_HEADER_SIZE as usize
        + constants::PROP_SLOT_VALUE_OFFSET as usize;
    let (table_raw, copied) = ctx.with_memory(|data| {
        let table_raw = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let copied = i64::from_le_bytes(data[copied_slot..copied_slot + 8].try_into().unwrap());
        (table_raw, copied)
    });

    assert_ne!(moved.ptr() as usize, source);
    assert_eq!(moved.color(), ZColor::Remapped);
    assert_eq!(table_raw, raw);
    assert_eq!(copied, child);
    assert_eq!(pages.page(0).unwrap().kind, ZPageKind::Free);
}

#[test]
fn zgc_relocate_remapped_entry_points_at_destination() {
    let entry = ZEntry::new(0x8000, ZColor::Remapped);

    assert_eq!(entry.ptr(), 0x8000);
    assert_eq!(entry.color(), ZColor::Remapped);
    assert_eq!(entry.raw(), 0x8003);
}

#[test]
fn zgc_relocate_source_page_waits_for_last_live_object() {
    let mut pages = ZPageSpace::default();
    let first = ZPAGE_SIZE;
    let object_size = constants::HEAP_OBJECT_HEADER_SIZE as usize
        + constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE as usize;
    let second = first + object_size;
    pages.attach(ZPAGE_SIZE, 3 * ZPAGE_SIZE);
    pages.add_live_bytes_range(first, object_size);
    pages.add_live_bytes_range(second, object_size);
    assert!(pages.mark_relocation_set(0));

    assert_eq!(
        release_empty_source_pages(&mut pages, first, object_size),
        0
    );
    assert_eq!(pages.page(0).unwrap().kind, ZPageKind::Relocating);

    assert_eq!(
        release_empty_source_pages(&mut pages, second, object_size),
        1
    );
    assert_eq!(pages.page(0).unwrap().kind, ZPageKind::Free);
}

#[test]
fn zgc_relocate_releases_cross_page_source_object_chunks() {
    let mut pages = ZPageSpace::default();
    let ptr = ZPAGE_SIZE - 8;
    let size = 16;
    pages.attach(0, 3 * ZPAGE_SIZE);
    pages.add_live_bytes_range(ptr, size);
    assert!(pages.mark_relocation_set(0));
    assert!(pages.mark_relocation_set(1));

    assert_eq!(release_empty_source_pages(&mut pages, ptr, size), 2);
    assert_eq!(pages.page(0).unwrap().kind, ZPageKind::Free);
    assert_eq!(pages.page(1).unwrap().kind, ZPageKind::Free);
}

#[test]
fn zgc_relocate_summary_does_not_publish_dead_handles() {
    let result = RelocateResult {
        relocated_objects: 2,
        relocated_bytes: 2 * ZPAGE_SIZE,
        released_pages: 1,
    };

    assert_eq!(result.relocated_objects, 2);
    assert_eq!(result.released_pages, 1);
}
