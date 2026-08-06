//! Regression tests for issues #111 (targeted arr proto remap) and #113 (reset before restore).

use wjsm_ir::constants::{
    HEAP_OBJECT_SHAPE_ID_OFFSET, HEAP_OBJECT_VALUE_CAPACITY_OFFSET, HEAP_OBJECT_VALUE_SLOT_SIZE,
};
use wjsm_ir::value;
use wjsm_ir::{HEAP_TYPE_ARRAY, HEAP_TYPE_OBJECT};
use wjsm_runtime::startup_snapshot_remap::remap_array_proto_function_indices;

#[test]
fn remap_touches_only_object_property_value_slots() -> anyhow::Result<()> {
    let snapshot_base = 100u32;
    let table_len = 2u32;
    let current_base = 200u32;

    // 新布局：对象 = 16 字节头 + value_capacity × 8 字节值槽；+8 值槽容量、
    // +12 shape_id。属性值直接写在值槽（16 + index * 8）里，堆内不再有
    // name_id/flags 字段。
    let mut heap = vec![0u8; 16 + HEAP_OBJECT_VALUE_SLOT_SIZE as usize];
    heap[4] = HEAP_TYPE_OBJECT;
    heap[8..12].copy_from_slice(&1u32.to_le_bytes()); // value_capacity = 1
    heap[12..16].copy_from_slice(&1u32.to_le_bytes()); // shape_id = 1
    let slot_off = 16usize; // 值槽 0
    let func_val = value::encode_function_idx(snapshot_base + 1);
    heap[slot_off..slot_off + 8].copy_from_slice(&func_val.to_le_bytes());

    // Metadata after object that looks like a function tag if scanned as i64 — must stay unchanged.
    let junk_off = heap.len();
    heap.extend_from_slice(&[0xFF; 8]);
    let junk_before = heap[junk_off..junk_off + 8].to_vec();

    remap_array_proto_function_indices(
        &mut heap[..16 + HEAP_OBJECT_VALUE_SLOT_SIZE as usize],
        snapshot_base,
        table_len,
        current_base,
    )?;

    let remapped = i64::from_le_bytes(heap[slot_off..slot_off + 8].try_into()?);
    assert_eq!(value::decode_function_idx(remapped), current_base + 1);
    assert_eq!(&heap[junk_off..junk_off + 8], junk_before.as_slice());

    Ok(())
}

#[test]
fn remap_skips_non_object_regions_in_heap_walk() -> anyhow::Result<()> {
    let snapshot_base = 10u32;
    let table_len = 1u32;
    let current_base = 20u32;

    // Array header only (no property slots); trailing bytes must not be remapped blindly.
    let mut heap = vec![0u8; 32];
    heap[4] = HEAP_TYPE_ARRAY;
    heap[8..12].copy_from_slice(&2u32.to_le_bytes());
    let fake_func = value::encode_function_idx(snapshot_base);
    heap[24..32].copy_from_slice(&fake_func.to_le_bytes());

    remap_array_proto_function_indices(&mut heap, snapshot_base, table_len, current_base)?;

    assert_eq!(
        i64::from_le_bytes(heap[24..32].try_into()?),
        fake_func,
        "array element storage must not be treated as property value slots"
    );
    Ok(())
}
