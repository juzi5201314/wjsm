//! Targeted Array.prototype function index remap (issue #111).

use anyhow::Result;
use wjsm_ir::constants::{
    FLAG_IS_ACCESSOR, HEAP_ARRAY_CAPACITY_OFFSET, HEAP_ARRAY_ELEMENT_SIZE,
    HEAP_OBJECT_CAPACITY_OFFSET, HEAP_OBJECT_HEADER_SIZE, HEAP_OBJECT_PROPERTY_SLOT_SIZE,
    HEAP_OBJECT_TYPE_OFFSET, PROP_SLOT_SIZE, PROP_SLOT_VALUE_OFFSET,
};
use wjsm_ir::value;
use wjsm_ir::{HEAP_TYPE_ARRAY, HEAP_TYPE_OBJECT};

/// Walk object property value slots in a restored heap slice; remap function indices
/// that fall in the seed module's Array.prototype wasm table range.
pub fn remap_array_proto_function_indices(
    data: &mut [u8],
    snapshot_base: u32,
    table_len: u32,
    current_base: u32,
) -> Result<()> {
    if snapshot_base == current_base || table_len == 0 {
        return Ok(());
    }

    let snapshot_end = snapshot_base
        .checked_add(table_len)
        .ok_or_else(|| anyhow::anyhow!("restore: Array.prototype table range overflow"))?;

    let heap_end = data.len();
    let mut ptr = 0usize;
    while ptr + HEAP_OBJECT_HEADER_SIZE as usize <= heap_end {
        let heap_type = data[ptr + HEAP_OBJECT_TYPE_OFFSET as usize];
        let (capacity_offset, elem_size) = if heap_type == HEAP_TYPE_ARRAY {
            (HEAP_ARRAY_CAPACITY_OFFSET, HEAP_ARRAY_ELEMENT_SIZE)
        } else if heap_type == HEAP_TYPE_OBJECT {
            (HEAP_OBJECT_CAPACITY_OFFSET, HEAP_OBJECT_PROPERTY_SLOT_SIZE)
        } else {
            ptr += 1;
            continue;
        };
        let cap_start = ptr + capacity_offset as usize;
        let capacity =
            u32::from_le_bytes(data[cap_start..cap_start + 4].try_into().expect("capacity"));
        let obj_size = (HEAP_OBJECT_HEADER_SIZE as usize)
            .saturating_add(capacity as usize * elem_size as usize);
        if obj_size == 0 || ptr.saturating_add(obj_size) > heap_end {
            break;
        }

        if heap_type == HEAP_TYPE_OBJECT {
            let props_base = ptr + HEAP_OBJECT_HEADER_SIZE as usize;
            for slot in 0..capacity as usize {
                let slot_off = props_base + slot * PROP_SLOT_SIZE as usize;
                if slot_off + PROP_SLOT_SIZE as usize > heap_end {
                    break;
                }
                let flags =
                    i32::from_le_bytes(data[slot_off + 4..slot_off + 8].try_into().expect("flags"));
                if flags & FLAG_IS_ACCESSOR != 0 {
                    continue;
                }
                let val_off = slot_off + PROP_SLOT_VALUE_OFFSET as usize;
                let raw = i64::from_le_bytes(data[val_off..val_off + 8].try_into().expect("val"));
                if !value::is_function(raw) {
                    continue;
                }
                let table_idx = value::decode_function_idx(raw);
                if table_idx < snapshot_base || table_idx >= snapshot_end {
                    continue;
                }
                let remapped =
                    value::encode_function_idx(current_base + (table_idx - snapshot_base));
                data[val_off..val_off + 8].copy_from_slice(&remapped.to_le_bytes());
            }
        }
        ptr += obj_size;
    }
    Ok(())
}
