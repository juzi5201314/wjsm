//! Regression tests for issues #111 (targeted arr proto remap) and #113 (reset before restore).

use wjsm_ir::constants::{FLAG_WRITABLE, PROP_SLOT_SIZE, PROP_SLOT_VALUE_OFFSET};
use wjsm_ir::value;
use wjsm_ir::{HEAP_TYPE_OBJECT, HEAP_TYPE_ARRAY};
use wjsm_runtime::startup_snapshot_remap::remap_array_proto_function_indices;

#[test]
fn remap_touches_only_object_property_value_slots() -> anyhow::Result<()> {
    let snapshot_base = 100u32;
    let table_len = 2u32;
    let current_base = 200u32;

    // Object: header 16 + one 32-byte property slot at rel 0.
    let mut heap = vec![0u8; 16 + PROP_SLOT_SIZE as usize];
    heap[4] = HEAP_TYPE_OBJECT;
    heap[8..12].copy_from_slice(&1u32.to_le_bytes()); // capacity = 1
    let slot_off = 16usize;
    heap[slot_off + 4..slot_off + 8].copy_from_slice(&(FLAG_WRITABLE).to_le_bytes());
    let func_val = value::encode_function_idx(snapshot_base + 1);
    heap[slot_off + PROP_SLOT_VALUE_OFFSET as usize..slot_off + PROP_SLOT_VALUE_OFFSET as usize + 8]
        .copy_from_slice(&func_val.to_le_bytes());

    // Metadata after object that looks like a function tag if scanned as i64 — must stay unchanged.
    let junk_off = heap.len();
    heap.extend_from_slice(&[0xFF; 8]);
    let junk_before = heap[junk_off..junk_off + 8].to_vec();

    remap_array_proto_function_indices(&mut heap[..16 + PROP_SLOT_SIZE as usize], snapshot_base, table_len, current_base)?;

    let remapped = i64::from_le_bytes(
        heap[slot_off + PROP_SLOT_VALUE_OFFSET as usize..slot_off + PROP_SLOT_VALUE_OFFSET as usize + 8]
            .try_into()?,
    );
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