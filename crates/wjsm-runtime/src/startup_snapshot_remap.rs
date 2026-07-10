//! Targeted Array.prototype function index remap (issue #111)。
//!
//! 实现委托给 [`crate::handle_remap`]：walker + [`FuncTableIndexRangePolicy`]。

use anyhow::Result;

use crate::handle_remap::{FuncTableIndexRangePolicy, walk_and_remap_heap};

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

    let _ = snapshot_base
        .checked_add(table_len)
        .ok_or_else(|| anyhow::anyhow!("restore: Array.prototype table range overflow"))?;

    walk_and_remap_heap(
        data,
        &FuncTableIndexRangePolicy {
            snapshot_base,
            table_len,
            current_base,
        },
    )
}
