use crate::{
    ArrayBufferEntry, RuntimeState, TypedArrayEntry, WasmEnv, alloc_host_object,
    typedarray_entry_from_value_with_env,
};
use wasmtime::AsContextMut;
use wjsm_ir::value;

pub(crate) fn transfer_byob_view_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    view: i64,
    bytes_written: usize,
) -> Option<i64> {
    let entry = typedarray_entry_from_value_with_env(ctx, env, view)?;
    let element_size = entry.element_size as usize;
    if element_size == 0 || !bytes_written.is_multiple_of(element_size) {
        return None;
    }
    let copied_bytes = {
        let start = entry.byte_offset as usize;
        let end = start.checked_add(bytes_written)?;
        let store = ctx.as_context_mut();
        if entry.is_shared {
            let shared = store.data().shared_state.as_ref()?.clone();
            let buffers = shared.sab_table.lock().ok()?;
            let buffer = buffers.get(entry.buffer_handle as usize)?;
            let data = buffer.data.read().ok()?;
            data.get(start..end)?.to_vec()
        } else {
            let buffers = store.data().arraybuffer_table.lock().ok()?;
            let buffer = buffers.get(entry.buffer_handle as usize)?;
            buffer.data.get(start..end)?.to_vec()
        }
    };
    let buffer_handle = {
        let store = ctx.as_context_mut();
        let mut buffers = store.data().arraybuffer_table.lock().ok()?;
        let handle = buffers.len() as u32;
        buffers.push(ArrayBufferEntry { data: copied_bytes });
        if !entry.is_shared
            && let Some(buffer) = buffers.get_mut(entry.buffer_handle as usize)
        {
            buffer.data.clear();
        }
        handle
    };
    let typedarray_handle = {
        let store = ctx.as_context_mut();
        let mut arrays = store.data().typedarray_table.lock().ok()?;
        let handle = arrays.len() as u32;
        arrays.push(TypedArrayEntry {
            buffer_handle,
            buffer_object: None,
            byte_offset: 0,
            length: (bytes_written / element_size) as u32,
            element_size: entry.element_size,
            element_kind: entry.element_kind,
            is_shared: false,
        });
        if !entry.is_shared {
            for typedarray in arrays.iter_mut() {
                if !typedarray.is_shared && typedarray.buffer_handle == entry.buffer_handle {
                    typedarray.byte_offset = 0;
                    typedarray.length = 0;
                }
            }
        }
        handle
    };
    if !entry.is_shared {
        let zero = value::encode_f64(0.0);
        for name in ["length", "byteLength", "byteOffset"] {
            let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
                ctx, env, view, name, zero,
            );
        }
    }
    let object = alloc_host_object(ctx, env, 8);
    for (name, raw) in [
        ("__typedarray_handle__", typedarray_handle),
        ("__arraybuffer_handle__", buffer_handle),
    ] {
        let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
            ctx,
            env,
            object,
            name,
            value::encode_f64(raw as f64),
        );
    }
    let length = value::encode_f64((bytes_written / element_size) as f64);
    let byte_length = value::encode_f64(bytes_written as f64);
    for (name, raw) in [
        ("length", length),
        ("byteLength", byte_length),
        ("byteOffset", value::encode_f64(0.0)),
    ] {
        let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
            ctx, env, object, name, raw,
        );
    }
    Some(object)
}
