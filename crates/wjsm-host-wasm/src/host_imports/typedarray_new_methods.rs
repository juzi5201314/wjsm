//! TypedArray 低层读写 + 薄 host 注册。
//!
//! 算法在 `wjsm_builtins::typedarray_methods`；本文件保留 ta_resolve/ta_read/ta_write
//! 等后端堆布局原语（供 ExecContext 与其他 host 模块使用）。

use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::exec_context_impl::WasmExecContext;
use crate::*;

/// 解析 TypedArray 的 this_val，返回 (buffer_handle, byte_offset, length, element_size, element_kind, is_shared)
pub(crate) fn ta_resolve(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> Option<(usize, usize, u32, u8, u8, bool)> {
    if !value::is_object(this_val) {
        return None;
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize)?;
    let h = read_object_property_by_name(caller, obj_ptr, "__typedarray_handle__")?;
    let handle = value::decode_f64(h) as usize;
    let table = caller.data().typedarray_table.lock().ok()?;
    let entry = table.get(handle)?;
    Some((
        entry.buffer_handle as usize,
        entry.byte_offset as usize,
        entry.length,
        entry.element_size,
        entry.element_kind,
        entry.is_shared,
    ))
}

/// 读取 TypedArray 第 index 个元素。
pub(crate) fn ta_read(
    caller: &mut Caller<'_, RuntimeState>,
    buf_handle: usize,
    byte_offset: usize,
    elem_size: u8,
    element_kind: u8,
    index: u32,
) -> Option<i64> {
    let table = caller.data().arraybuffer_table.lock().ok()?;
    let entry = table.get(buf_handle)?;
    let off = byte_offset + (index as usize) * (elem_size as usize);
    if off + (elem_size as usize) > entry.data.len() {
        return None;
    }
    let val = match (elem_size, element_kind) {
        (1, 0) => entry.data[off] as i8 as f64,
        (1, 1) | (1, 2) => entry.data[off] as f64,
        (2, 0) => i16::from_le_bytes([entry.data[off], entry.data[off + 1]]) as f64,
        (2, 1) => u16::from_le_bytes([entry.data[off], entry.data[off + 1]]) as f64,
        (4, 0) => i32::from_le_bytes([
            entry.data[off],
            entry.data[off + 1],
            entry.data[off + 2],
            entry.data[off + 3],
        ]) as f64,
        (4, 1) => u32::from_le_bytes([
            entry.data[off],
            entry.data[off + 1],
            entry.data[off + 2],
            entry.data[off + 3],
        ]) as f64,
        (4, 3) => f32::from_le_bytes([
            entry.data[off],
            entry.data[off + 1],
            entry.data[off + 2],
            entry.data[off + 3],
        ]) as f64,
        (8, 3) => f64::from_le_bytes([
            entry.data[off],
            entry.data[off + 1],
            entry.data[off + 2],
            entry.data[off + 3],
            entry.data[off + 4],
            entry.data[off + 5],
            entry.data[off + 6],
            entry.data[off + 7],
        ]),
        (8, 4) => {
            let v = i64::from_le_bytes([
                entry.data[off],
                entry.data[off + 1],
                entry.data[off + 2],
                entry.data[off + 3],
                entry.data[off + 4],
                entry.data[off + 5],
                entry.data[off + 6],
                entry.data[off + 7],
            ]);
            let mut table = caller.data().bigint_table.lock().ok()?;
            let handle = table.len() as u32;
            table.push(num_bigint::BigInt::from(v));
            return Some(value::encode_bigint_handle(handle));
        }
        (8, 5) => {
            let v = u64::from_le_bytes([
                entry.data[off],
                entry.data[off + 1],
                entry.data[off + 2],
                entry.data[off + 3],
                entry.data[off + 4],
                entry.data[off + 5],
                entry.data[off + 6],
                entry.data[off + 7],
            ]);
            let mut table = caller.data().bigint_table.lock().ok()?;
            let handle = table.len() as u32;
            table.push(num_bigint::BigInt::from(v));
            return Some(value::encode_bigint_handle(handle));
        }
        _ => return None,
    };
    Some(value::encode_f64(val))
}

/// 写入 TypedArray 第 index 个元素。
pub(crate) fn ta_write(
    caller: &mut Caller<'_, RuntimeState>,
    buf_handle: usize,
    byte_offset: usize,
    elem_size: u8,
    element_kind: u8,
    index: u32,
    val: i64,
) -> Option<()> {
    let bytes = encode_typedarray_element(caller, elem_size, element_kind, val)?;
    let mut table = caller.data().arraybuffer_table.lock().ok()?;
    let entry = table.get_mut(buf_handle)?;
    let off = byte_offset + (index as usize) * (elem_size as usize);
    if off + (elem_size as usize) > entry.data.len() {
        return None;
    }
    entry.data[off..off + elem_size as usize].copy_from_slice(&bytes[..elem_size as usize]);
    Some(())
}

/// 从 SharedArrayBuffer 读取。
pub(crate) fn sab_read(
    caller: &mut Caller<'_, RuntimeState>,
    buf_handle: usize,
    byte_offset: usize,
    elem_size: u8,
    element_kind: u8,
    index: u32,
) -> Option<i64> {
    let shared = caller.data().shared_state.as_ref()?;
    let sab_table = shared.sab_table.lock().ok()?;
    let entry = sab_table.get(buf_handle)?;
    let data = entry.data.read().ok()?;
    let off = byte_offset + (index as usize) * (elem_size as usize);
    if off + (elem_size as usize) > data.len() {
        return None;
    }
    let val = match (elem_size, element_kind) {
        (1, 0) => data[off] as i8 as f64,
        (1, 1) | (1, 2) => data[off] as f64,
        (2, 0) => i16::from_le_bytes([data[off], data[off + 1]]) as f64,
        (2, 1) => u16::from_le_bytes([data[off], data[off + 1]]) as f64,
        (4, 0) => {
            i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as f64
        }
        (4, 1) => {
            u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as f64
        }
        (4, 3) => {
            f32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as f64
        }
        (8, 3) => f64::from_le_bytes([
            data[off],
            data[off + 1],
            data[off + 2],
            data[off + 3],
            data[off + 4],
            data[off + 5],
            data[off + 6],
            data[off + 7],
        ]),
        (8, 4) => {
            let v = i64::from_le_bytes([
                data[off],
                data[off + 1],
                data[off + 2],
                data[off + 3],
                data[off + 4],
                data[off + 5],
                data[off + 6],
                data[off + 7],
            ]);
            let mut table = caller.data().bigint_table.lock().ok()?;
            let handle = table.len() as u32;
            table.push(num_bigint::BigInt::from(v));
            return Some(value::encode_bigint_handle(handle));
        }
        (8, 5) => {
            let v = u64::from_le_bytes([
                data[off],
                data[off + 1],
                data[off + 2],
                data[off + 3],
                data[off + 4],
                data[off + 5],
                data[off + 6],
                data[off + 7],
            ]);
            let mut table = caller.data().bigint_table.lock().ok()?;
            let handle = table.len() as u32;
            table.push(num_bigint::BigInt::from(v));
            return Some(value::encode_bigint_handle(handle));
        }
        _ => return None,
    };
    Some(value::encode_f64(val))
}

/// 写入 SharedArrayBuffer TypedArray。
pub(crate) fn sab_write(
    caller: &mut Caller<'_, RuntimeState>,
    buf_handle: usize,
    byte_offset: usize,
    elem_size: u8,
    element_kind: u8,
    index: u32,
    val: i64,
) -> Option<()> {
    let bytes = encode_typedarray_element(caller, elem_size, element_kind, val)?;
    let shared = caller.data().shared_state.as_ref()?;
    let sab_table = shared.sab_table.lock().ok()?;
    let entry = sab_table.get(buf_handle)?;
    let mut data = entry.data.write().ok()?;
    let off = byte_offset + (index as usize) * (elem_size as usize);
    if off + (elem_size as usize) > data.len() {
        return None;
    }
    data[off..off + elem_size as usize].copy_from_slice(&bytes[..elem_size as usize]);
    Some(())
}

pub(crate) fn define_typedarray_new_methods(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let fill = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         value: i64,
         start_raw: i64,
         end_raw: i64|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::typedarray_methods::typedarray_proto_fill(
                &mut ctx, this_val, value, start_raw, end_raw,
            )
        },
    );
    linker.define(&mut store, "env", "typedarray_proto_fill", fill)?;

    let reverse = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::typedarray_methods::typedarray_proto_reverse(&mut ctx, this_val)
        },
    );
    linker.define(&mut store, "env", "typedarray_proto_reverse", reverse)?;

    let index_of = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         search: i64,
         from_index: i64|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::typedarray_methods::typedarray_proto_index_of(
                &mut ctx, this_val, search, from_index,
            )
        },
    );
    linker.define(&mut store, "env", "typedarray_proto_index_of", index_of)?;

    let last_index_of = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         search: i64,
         from_index: i64|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::typedarray_methods::typedarray_proto_last_index_of(
                &mut ctx, this_val, search, from_index,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_last_index_of",
        last_index_of,
    )?;

    let includes = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         search: i64,
         from_index: i64|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::typedarray_methods::typedarray_proto_includes(
                &mut ctx, this_val, search, from_index,
            )
        },
    );
    linker.define(&mut store, "env", "typedarray_proto_includes", includes)?;

    let join = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, sep: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::typedarray_methods::typedarray_proto_join(&mut ctx, this_val, sep)
        },
    );
    linker.define(&mut store, "env", "typedarray_proto_join", join)?;

    let to_string = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::typedarray_methods::typedarray_proto_to_string(&mut ctx, this_val)
        },
    );
    linker.define(&mut store, "env", "typedarray_proto_to_string", to_string)?;

    let copy_within = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         target: i64,
         start: i64,
         end: i64|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::typedarray_methods::typedarray_proto_copy_within(
                &mut ctx, this_val, target, start, end,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_copy_within",
        copy_within,
    )?;

    let at = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::typedarray_methods::typedarray_proto_at(&mut ctx, this_val, index)
        },
    );
    linker.define(&mut store, "env", "typedarray_proto_at", at)?;

    // live TypedArray 迭代器（IteratorState::TypedArray*Iter）
    let typedarray_proto_entries_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let Some(entry) = typedarray_entry_from_value(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let length = entry.length;
            let handle;
            {
                let mut iter_table = caller
                    .data()
                    .iterators
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                handle = iter_table.len() as u32;
                iter_table.push(IteratorState::TypedArrayEntryIter {
                    entry,
                    index: 0,
                    length,
                });
            }
            value::encode_handle(value::TAG_ITERATOR, handle)
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_entries",
        typedarray_proto_entries_fn,
    )?;

    let typedarray_proto_keys_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let (_buf_handle, _byte_offset, length, _elem_size, _element_kind, _is_shared) =
                match ta_resolve(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let values = (0..length).map(|i| value::encode_f64(i as f64)).collect();
            let handle;
            {
                let mut iter_table = caller
                    .data()
                    .iterators
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                handle = iter_table.len() as u32;
                iter_table.push(IteratorState::IndexValueIter { values, index: 0 });
            }
            value::encode_handle(value::TAG_ITERATOR, handle)
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_keys",
        typedarray_proto_keys_fn,
    )?;

    let typedarray_proto_values_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let Some(entry) = typedarray_entry_from_value(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let length = entry.length;
            let handle;
            {
                let mut iter_table = caller
                    .data()
                    .iterators
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                handle = iter_table.len() as u32;
                iter_table.push(IteratorState::TypedArrayValueIter {
                    entry,
                    index: 0,
                    length,
                });
            }
            value::encode_handle(value::TAG_ITERATOR, handle)
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_values",
        typedarray_proto_values_fn,
    )?;

    Ok(())
}
