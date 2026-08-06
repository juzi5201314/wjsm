//! handle_remap walker + RemapPolicy 内核测试（Task 0.2）。
//!
//! 隐藏类重构后堆内 payload 只有「16 字节 header + N×8 值槽」一种形态：
//! 属性名与 flags 都在宿主 `ShapeTable`，堆里每槽统一是一个 boxed i64。
//! 因此这里伪造的对象不再写 name_id/flags，accessor 只是两个相邻值槽
//! （getter@index、setter@index+1）。

use wjsm_ir::constants::{
    HEAP_OBJECT_HEADER_SIZE, HEAP_OBJECT_PROTO_OFFSET, HEAP_OBJECT_SHAPE_ID_OFFSET,
    HEAP_OBJECT_TYPE_OFFSET, HEAP_OBJECT_VALUE_CAPACITY_OFFSET, HEAP_OBJECT_VALUE_SLOT_SIZE,
    SHAPE_ID_EMPTY,
};
use wjsm_ir::value;
use wjsm_ir::{HEAP_TYPE_ARRAY, HEAP_TYPE_OBJECT};
use wjsm_runtime::startup_snapshot_remap::remap_array_proto_function_indices;
use wjsm_runtime::{
    FuncTableIndexRangePolicy, HandleMap, ObjectHandleMapPolicy, walk_and_remap_heap,
};

/// 构造 OBJECT：header + `capacity` 个值槽（预填 0）。
fn alloc_object_heap(capacity: u32, proto: u32) -> Vec<u8> {
    let size = HEAP_OBJECT_HEADER_SIZE as usize
        + capacity as usize * HEAP_OBJECT_VALUE_SLOT_SIZE as usize;
    let mut heap = vec![0u8; size];
    heap[HEAP_OBJECT_PROTO_OFFSET as usize..HEAP_OBJECT_PROTO_OFFSET as usize + 4]
        .copy_from_slice(&proto.to_le_bytes());
    heap[HEAP_OBJECT_TYPE_OFFSET as usize] = HEAP_TYPE_OBJECT;
    heap[HEAP_OBJECT_VALUE_CAPACITY_OFFSET as usize..HEAP_OBJECT_VALUE_CAPACITY_OFFSET as usize + 4]
        .copy_from_slice(&capacity.to_le_bytes());
    heap[HEAP_OBJECT_SHAPE_ID_OFFSET as usize..HEAP_OBJECT_SHAPE_ID_OFFSET as usize + 4]
        .copy_from_slice(&SHAPE_ID_EMPTY.to_le_bytes());
    heap
}

fn slot_off(index: usize) -> usize {
    HEAP_OBJECT_HEADER_SIZE as usize + index * HEAP_OBJECT_VALUE_SLOT_SIZE as usize
}

fn write_slot(heap: &mut [u8], index: usize, val: i64) {
    let off = slot_off(index);
    heap[off..off + 8].copy_from_slice(&val.to_le_bytes());
}

fn read_slot(heap: &[u8], index: usize) -> i64 {
    read_i64(heap, slot_off(index))
}

fn read_i64(heap: &[u8], off: usize) -> i64 {
    i64::from_le_bytes(heap[off..off + 8].try_into().unwrap())
}

fn read_proto(heap: &[u8]) -> u32 {
    u32::from_le_bytes(heap[0..4].try_into().unwrap())
}

#[test]
fn policy_a_func_table_remaps_function_idx_not_object_handle() -> anyhow::Result<()> {
    let snapshot_base = 100u32;
    let table_len = 2u32;
    let current_base = 200u32;

    let mut heap = alloc_object_heap(2, 5);
    write_slot(&mut heap, 0, value::encode_function_idx(snapshot_base + 1));
    write_slot(&mut heap, 1, value::encode_object_handle(7));

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
        value::decode_function_idx(read_slot(&heap, 0)),
        current_base + 1
    );
    assert_eq!(
        value::decode_object_handle(read_slot(&heap, 1)),
        7,
        "object handle 不变"
    );
    Ok(())
}

#[test]
fn policy_a_leaves_function_idx_outside_snapshot_range_untouched() -> anyhow::Result<()> {
    let snapshot_base = 10u32;
    let table_len = 2u32;
    let current_base = 20u32;

    let mut heap = alloc_object_heap(2, u32::MAX);
    // 区间内 → 平移；区间外 → 原样。
    let inside = value::encode_function_idx(snapshot_base + 1);
    let outside = value::encode_function_idx(snapshot_base + table_len);
    write_slot(&mut heap, 0, inside);
    write_slot(&mut heap, 1, outside);

    remap_array_proto_function_indices(&mut heap, snapshot_base, table_len, current_base)?;

    assert_eq!(
        value::decode_function_idx(read_slot(&heap, 0)),
        current_base + 1
    );
    assert_eq!(read_slot(&heap, 1), outside, "区间外的函数索引不得改写");
    Ok(())
}

#[test]
fn policy_a_remaps_accessor_value_slots() -> anyhow::Result<()> {
    let snapshot_base = 10u32;
    let table_len = 1u32;
    let current_base = 20u32;

    let mut heap = alloc_object_heap(2, u32::MAX);
    // accessor = 两个相邻值槽；getter 落在 snapshot 区间内。
    write_slot(&mut heap, 0, value::encode_function_idx(snapshot_base));
    write_slot(&mut heap, 1, value::encode_undefined());

    remap_array_proto_function_indices(&mut heap, snapshot_base, table_len, current_base)?;

    // 堆内已无 flags，getter 与数据属性一样是值槽——种子模块的 getter 函数索引
    // 同样必须平移到 current_base，否则它会指向错误的 wasm 表项。
    assert_eq!(value::decode_function_idx(read_slot(&heap, 0)), current_base);
    assert_eq!(
        read_slot(&heap, 1),
        value::encode_undefined(),
        "setter 槽仍是 undefined"
    );
    Ok(())
}

#[test]
fn policy_b_object_handle_map_rewrites_proto_and_every_value_slot() -> anyhow::Result<()> {
    let mut map = HandleMap::new();
    map.insert(5, 105);
    map.insert(7, 107);
    map.insert(8, 108);
    map.insert(9, 109);

    let mut heap = alloc_object_heap(5, 5);
    write_slot(&mut heap, 0, value::encode_object_handle(7));
    // accessor：getter@1 / setter@2
    write_slot(&mut heap, 1, value::encode_object_handle(8));
    write_slot(&mut heap, 2, value::encode_object_handle(9));
    write_slot(&mut heap, 3, value::encode_f64(3.25));
    write_slot(&mut heap, 4, value::encode_object_handle(11)); // 未映射

    walk_and_remap_heap(&mut heap, &ObjectHandleMapPolicy { map: &map })?;

    assert_eq!(read_proto(&heap), 105);
    assert_eq!(value::decode_object_handle(read_slot(&heap, 0)), 107);
    assert_eq!(value::decode_object_handle(read_slot(&heap, 1)), 108);
    assert_eq!(value::decode_object_handle(read_slot(&heap, 2)), 109);
    assert!(
        (value::decode_f64(read_slot(&heap, 3)) - 3.25).abs() < 1e-9,
        "number 不变"
    );
    assert_eq!(
        value::decode_object_handle(read_slot(&heap, 4)),
        11,
        "未映射 handle 保持原样"
    );

    // function table idx 不被 ObjectHandleMap 改写
    let mut fn_heap = alloc_object_heap(1, 5);
    let fn_val = value::encode_function_idx(42);
    write_slot(&mut fn_heap, 0, fn_val);
    walk_and_remap_heap(&mut fn_heap, &ObjectHandleMapPolicy { map: &map })?;
    assert_eq!(read_slot(&fn_heap, 0), fn_val);

    Ok(())
}

#[test]
fn policy_b_rewrites_array_element_handles() -> anyhow::Result<()> {
    let mut map = HandleMap::new();
    map.insert(3, 33);

    let mut heap = vec![0u8; 16 + 8];
    heap[HEAP_OBJECT_TYPE_OFFSET as usize] = HEAP_TYPE_ARRAY;
    heap[8..12].copy_from_slice(&1u32.to_le_bytes()); // length = 1
    heap[12..16].copy_from_slice(&1u32.to_le_bytes()); // capacity = 1
    heap[16..24].copy_from_slice(&value::encode_object_handle(3).to_le_bytes());

    walk_and_remap_heap(&mut heap, &ObjectHandleMapPolicy { map: &map })?;

    assert_eq!(value::decode_object_handle(read_i64(&heap, 16)), 33);
    Ok(())
}

/// 对象与数组的容量字段位置不同（对象 `+8`，数组 `+12`），walker 必须按
/// heap_type 取对应字段，否则会算错步长并把后续对象错位解释。
#[test]
fn walker_advances_correctly_across_mixed_object_and_array() -> anyhow::Result<()> {
    let mut map = HandleMap::new();
    map.insert(3, 33);
    map.insert(4, 44);

    // 对象（2 值槽，容量在 +8）在前，数组（1 元素，容量在 +12）紧随其后。
    let mut heap = alloc_object_heap(2, u32::MAX);
    write_slot(&mut heap, 0, value::encode_object_handle(3));
    write_slot(&mut heap, 1, value::encode_undefined());
    let array_ptr = heap.len();
    heap.extend(std::iter::repeat_n(0u8, 16 + 8));
    heap[array_ptr + HEAP_OBJECT_TYPE_OFFSET as usize] = HEAP_TYPE_ARRAY;
    heap[array_ptr + 8..array_ptr + 12].copy_from_slice(&1u32.to_le_bytes());
    heap[array_ptr + 12..array_ptr + 16].copy_from_slice(&1u32.to_le_bytes());
    heap[array_ptr + 16..array_ptr + 24]
        .copy_from_slice(&value::encode_object_handle(4).to_le_bytes());

    walk_and_remap_heap(&mut heap, &ObjectHandleMapPolicy { map: &map })?;

    assert_eq!(value::decode_object_handle(read_slot(&heap, 0)), 33);
    assert_eq!(
        value::decode_object_handle(read_i64(&heap, array_ptr + 16)),
        44,
        "数组必须被走到，说明对象步长按 +8 的值槽容量算对了"
    );
    Ok(())
}

#[test]
fn legacy_remap_api_delegates_to_walker() -> anyhow::Result<()> {
    let snapshot_base = 100u32;
    let table_len = 2u32;
    let current_base = 200u32;
    let mut heap = alloc_object_heap(1, u32::MAX);
    write_slot(&mut heap, 0, value::encode_function_idx(snapshot_base + 1));
    remap_array_proto_function_indices(&mut heap, snapshot_base, table_len, current_base)?;
    assert_eq!(
        value::decode_function_idx(read_slot(&heap, 0)),
        current_base + 1
    );
    Ok(())
}
