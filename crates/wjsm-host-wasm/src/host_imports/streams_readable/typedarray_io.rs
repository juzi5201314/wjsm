use crate::{RuntimeState, typedarray_entry_from_value};
use wasmtime::Caller;

pub(crate) fn typedarray_u8_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    typedarray: i64,
) -> Option<Vec<u8>> {
    let entry = typedarray_entry_from_value(caller, typedarray)?;
    if entry.element_size != 1 {
        return None;
    }
    let start = entry.byte_offset as usize;
    let length = entry.length as usize;
    if entry.is_shared {
        let shared = caller.data().shared_state.as_ref()?.clone();
        let buffers = shared.sab_table.lock().ok()?;
        let buffer = buffers.get(entry.buffer_handle as usize)?;
        let data = buffer.data.read().ok()?;
        let end = start.checked_add(length)?;
        data.get(start..end).map(<[u8]>::to_vec)
    } else {
        let buffers = caller.data().arraybuffer_table.lock().ok()?;
        let buffer = buffers.get(entry.buffer_handle as usize)?;
        let end = start.checked_add(length)?;
        buffer.data.get(start..end).map(<[u8]>::to_vec)
    }
}

pub(crate) fn write_u8_bytes_to_view(
    caller: &mut Caller<'_, RuntimeState>,
    view: i64,
    bytes: &[u8],
) -> Option<usize> {
    let entry = typedarray_entry_from_value(caller, view)?;
    if entry.element_size != 1 {
        return None;
    }
    let write_length = (entry.length as usize).min(bytes.len());
    let start = entry.byte_offset as usize;
    if entry.is_shared {
        let shared = caller.data().shared_state.as_ref()?.clone();
        let buffers = shared.sab_table.lock().ok()?;
        let buffer = buffers.get(entry.buffer_handle as usize)?;
        let mut data = buffer.data.write().ok()?;
        let end = start.checked_add(write_length)?;
        data.get_mut(start..end)?
            .copy_from_slice(&bytes[..write_length]);
    } else {
        let mut buffers = caller.data().arraybuffer_table.lock().ok()?;
        let buffer = buffers.get_mut(entry.buffer_handle as usize)?;
        let end = start.checked_add(write_length)?;
        buffer
            .data
            .get_mut(start..end)?
            .copy_from_slice(&bytes[..write_length]);
    }
    Some(write_length)
}
