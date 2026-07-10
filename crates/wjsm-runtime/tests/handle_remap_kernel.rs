//! handle_remap walker + RemapPolicy 内核测试（Task 0.2）。

use wjsm_ir::constants::{
    FLAG_IS_ACCESSOR, FLAG_WRITABLE, HEAP_OBJECT_HEADER_SIZE, HEAP_OBJECT_PROTO_OFFSET,
    HEAP_OBJECT_TYPE_OFFSET, PROP_SLOT_FLAGS_OFFSET, PROP_SLOT_GETTER_OFFSET,
    PROP_SLOT_SETTER_OFFSET, PROP_SLOT_SIZE, PROP_SLOT_VALUE_OFFSET,
};
use wjsm_ir::value;
use wjsm_ir::{HEAP_TYPE_ARRAY, HEAP_TYPE_OBJECT};
use wjsm_runtime::startup_snapshot_remap::remap_array_proto_function_indices;
use wjsm_runtime::{
    FuncTableIndexRangePolicy, HandleMap, ObjectHandleMapPolicy, walk_and_remap_heap,
};

/// 构造 OBJECT：header + N 个属性槽（预填 0）。
fn alloc_object_heap(capacity: u32, proto: u32) -> Vec<u8> {
    let size = HEAP_OBJECT_HEADER_SIZE as usize + capacity as usize * PROP_SLOT_SIZE as usize;
    let mut heap = vec![0u8; size];
    heap[HEAP_OBJECT_PROTO_OFFSET as usize..HEAP_OBJECT_PROTO_OFFSET as usize + 4]
        .copy_from_slice(&proto.to_le_bytes());
    heap[HEAP_OBJECT_TYPE_OFFSET as usize] = HEAP_TYPE_OBJECT;
    heap[8..12].copy_from_slice(&capacity.to_le_bytes());
    heap[12..16].copy_from_slice(&capacity.to_le_bytes());
    heap
}

fn write_data_prop(heap: &mut [u8], slot: usize, val: i64) {
    let slot_off = HEAP_OBJECT_HEADER_SIZE as usize + slot * PROP_SLOT_SIZE as usize;
    heap[slot_off + PROP_SLOT_FLAGS_OFFSET as usize
        ..slot_off + PROP_SLOT_FLAGS_OFFSET as usize + 4]
        .copy_from_slice(&FLAG_WRITABLE.to_le_bytes());
    heap[slot_off + PROP_SLOT_VALUE_OFFSET as usize
        ..slot_off + PROP_SLOT_VALUE_OFFSET as usize + 8]
        .copy_from_slice(&val.to_le_bytes());
}

fn write_accessor_prop(heap: &mut [u8], slot: usize, getter: i64, setter: i64) {
    let slot_off = HEAP_OBJECT_HEADER_SIZE as usize + slot * PROP_SLOT_SIZE as usize;
    heap[slot_off + PROP_SLOT_FLAGS_OFFSET as usize
        ..slot_off + PROP_SLOT_FLAGS_OFFSET as usize + 4]
        .copy_from_slice(&FLAG_IS_ACCESSOR.to_le_bytes());
    heap[slot_off + PROP_SLOT_GETTER_OFFSET as usize
        ..slot_off + PROP_SLOT_GETTER_OFFSET as usize + 8]
        .copy_from_slice(&getter.to_le_bytes());
    heap[slot_off + PROP_SLOT_SETTER_OFFSET as usize
        ..slot_off + PROP_SLOT_SETTER_OFFSET as usize + 8]
        .copy_from_slice(&setter.to_le_bytes());
}

fn read_i64(heap: &[u8], off: usize) -> i64 {
    i64::from_le_bytes(heap[off..off + 8].try_into().unwrap())
}

fn read_proto(heap: &[u8]) -> u32 {
    u32::from_le_bytes(heap[0..4].try_into().unwrap())
}

fn slot_value_off(slot: usize) -> usize {
    HEAP_OBJECT_HEADER_SIZE as usize + slot * PROP_SLOT_SIZE as usize + PROP_SLOT_VALUE_OFFSET as usize
}

fn slot_getter_off(slot: usize) -> usize {
    HEAP_OBJECT_HEADER_SIZE as usize
        + slot * PROP_SLOT_SIZE as usize
        + PROP_SLOT_GETTER_OFFSET as usize
}

fn slot_setter_off(slot: usize) -> usize {
    HEAP_OBJECT_HEADER_SIZE as usize
        + slot * PROP_SLOT_SIZE as usize
        + PROP_SLOT_SETTER_OFFSET as usize
}

#[test]
fn policy_a_func_table_remaps_function_idx_not_object_handle() -> anyhow::Result<()> {
    let snapshot_base = 100u32;
    let table_len = 2u32;
    let current_base = 200u32;

    let mut heap = alloc_object_heap(2, 5);
    write_data_prop(&mut heap, 0, value::encode_function_idx(snapshot_base + 1));
    write_data_prop(&mut heap, 1, value::encode_object_handle(7));

    walk_and_remap_heap(
        &mut heap,
        &FuncTableIndexRangePolicy {
            snapshot_base,
            table_len,
            current_base,
        },
    )?;

    assert_eq!(read_proto(&heap), 5, "proto 不变");
    assert_eq!(
        value::decode_function_idx(read_i64(&heap, slot_value_off(0))),
        current_base + 1
    );
    assert_eq!(
        value::decode_object_handle(read_i64(&heap, slot_value_off(1))),
        7,
        "object handle 不变"
    );
    Ok(())
}

#[test]
fn policy_a_skips_accessor_slots_like_legacy() -> anyhow::Result<()> {
    let snapshot_base = 10u32;
    let table_len = 1u32;
    let current_base = 20u32;

    let mut heap = alloc_object_heap(1, u32::MAX);
    let getter = value::encode_function_idx(snapshot_base);
    write_accessor_prop(&mut heap, 0, getter, value::encode_undefined());

    remap_array_proto_function_indices(&mut heap, snapshot_base, table_len, current_base)?;

    assert_eq!(
        read_i64(&heap, slot_getter_off(0)),
        getter,
        "FuncTable policy 跳过 accessor"
    );
    Ok(())
}

#[test]
fn policy_b_object_handle_map_rewrites_proto_value_getter_setter() -> anyhow::Result<()> {
    let mut map = HandleMap::new();
    map.insert(5, 105);
    map.insert(7, 107);
    map.insert(8, 108);
    map.insert(9, 109);

    let mut heap = alloc_object_heap(4, 5);
    write_data_prop(&mut heap, 0, value::encode_object_handle(7));
    write_accessor_prop(
        &mut heap,
        1,
        value::encode_object_handle(8),
        value::encode_object_handle(9),
    );
    write_data_prop(&mut heap, 2, value::encode_f64(3.14));
    write_data_prop(&mut heap, 3, value::encode_object_handle(11)); // 未映射

    walk_and_remap_heap(&mut heap, &ObjectHandleMapPolicy { map: &map })?;

    assert_eq!(read_proto(&heap), 105);
    assert_eq!(
        value::decode_object_handle(read_i64(&heap, slot_value_off(0))),
        107
    );
    assert_eq!(
        value::decode_object_handle(read_i64(&heap, slot_getter_off(1))),
        108
    );
    assert_eq!(
        value::decode_object_handle(read_i64(&heap, slot_setter_off(1))),
        109
    );
    assert!(
        (value::decode_f64(read_i64(&heap, slot_value_off(2))) - 3.14).abs() < 1e-9,
        "number 不变"
    );
    assert_eq!(
        value::decode_object_handle(read_i64(&heap, slot_value_off(3))),
        11,
        "未映射 handle 保持原样"
    );

    // function table idx 不被 ObjectHandleMap 改写
    let mut fn_heap = alloc_object_heap(1, 5);
    let fn_val = value::encode_function_idx(42);
    write_data_prop(&mut fn_heap, 0, fn_val);
    walk_and_remap_heap(&mut fn_heap, &ObjectHandleMapPolicy { map: &map })?;
    assert_eq!(read_i64(&fn_heap, slot_value_off(0)), fn_val);

    Ok(())
}

#[test]
fn policy_b_rewrites_array_element_handles() -> anyhow::Result<()> {
    let mut map = HandleMap::new();
    map.insert(3, 33);

    let mut heap = vec![0u8; 16 + 8];
    heap[4] = HEAP_TYPE_ARRAY;
    heap[8..12].copy_from_slice(&1u32.to_le_bytes());
    heap[12..16].copy_from_slice(&1u32.to_le_bytes()); // capacity = 1
    heap[16..24].copy_from_slice(&value::encode_object_handle(3).to_le_bytes());

    walk_and_remap_heap(&mut heap, &ObjectHandleMapPolicy { map: &map })?;

    assert_eq!(value::decode_object_handle(read_i64(&heap, 16)), 33);
    Ok(())
}

#[test]
fn legacy_remap_api_delegates_to_walker() -> anyhow::Result<()> {
    let snapshot_base = 100u32;
    let table_len = 2u32;
    let current_base = 200u32;
    let mut heap = alloc_object_heap(1, u32::MAX);
    write_data_prop(
        &mut heap,
        0,
        value::encode_function_idx(snapshot_base + 1),
    );
    remap_array_proto_function_indices(&mut heap, snapshot_base, table_len, current_base)?;
    assert_eq!(
        value::decode_function_idx(read_i64(&heap, slot_value_off(0))),
        current_base + 1
    );
    Ok(())
}
