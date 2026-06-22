use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

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

/// 读取 TypedArray 第 index 个元素，返回 NaN-boxed f64 值。
/// 根据 element_kind 区分整数/无符号/浮点读写。
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

/// 写入 TypedArray 第 index 个元素，根据 element_kind 采用对应的整数/浮点编码。
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

/// 从 SharedArrayBuffer 读取 TypedArray 第 index 个元素
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

/// 写入 SharedArrayBuffer TypedArray 第 index 个元素
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
    // ── typedarray_proto_fill (Type 17, 4-arg: this, value, start, end) ──
    let typedarray_proto_fill_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         value: i64,
         start_raw: i64,
         end_raw: i64|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
                match ta_resolve(&mut caller, this_val) {
                    Some(v) => v,
                    None => return this_val,
                };
            let start = if value::is_undefined(start_raw) {
                0u32
            } else {
                let f = value::decode_f64(start_raw);
                if f < 0.0 {
                    (length as i32 + (f as i32)).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            let end = if value::is_undefined(end_raw) {
                length
            } else {
                let f = value::decode_f64(end_raw);
                if f < 0.0 {
                    (length as i32 + (f as i32)).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            for i in start..end {
                if is_shared {
                    sab_write(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                        value,
                    )
                } else {
                    ta_write(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                        value,
                    )
                };
            }
            this_val
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_fill",
        typedarray_proto_fill_fn,
    )?;

    // ── typedarray_proto_reverse (Type 3, 1-arg: this) ──
    let typedarray_proto_reverse_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
                match ta_resolve(&mut caller, this_val) {
                    Some(v) => v,
                    None => return this_val,
                };
            for i in 0..length / 2 {
                let a = if is_shared {
                    sab_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                    )
                } else {
                    ta_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                    )
                }
                .unwrap_or(value::encode_undefined());
                let b = if is_shared {
                    sab_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        length - 1 - i,
                    )
                } else {
                    ta_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        length - 1 - i,
                    )
                }
                .unwrap_or(value::encode_undefined());
                if is_shared {
                    sab_write(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                        b,
                    )
                } else {
                    ta_write(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                        b,
                    )
                };
                if is_shared {
                    sab_write(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        length - 1 - i,
                        a,
                    )
                } else {
                    ta_write(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        length - 1 - i,
                        a,
                    )
                };
            }
            this_val
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_reverse",
        typedarray_proto_reverse_fn,
    )?;

    // ── typedarray_proto_index_of (Type 16, 3-arg: this, searchElement, fromIndex) ──
    let typedarray_proto_index_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         search_element: i64,
         from_index: i64|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
                match ta_resolve(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_f64(-1.0),
                };
            let from_idx = if value::is_undefined(from_index) {
                0i32
            } else {
                let f = value::decode_f64(from_index);
                if f < 0.0 {
                    length as i32 + (f as i32).max(0)
                } else {
                    (f as i32).min(length as i32)
                }
            };
            for i in from_idx as u32..length {
                let elem = if is_shared {
                    sab_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                    )
                } else {
                    ta_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                    )
                }
                .unwrap_or(value::encode_undefined());
                if typedarray_same_value_zero(&mut caller, elem, search_element) {
                    return value::encode_f64(i as f64);
                }
            }
            value::encode_f64(-1.0)
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_index_of",
        typedarray_proto_index_of_fn,
    )?;

    // ── typedarray_proto_last_index_of (Type 16, 3-arg: this, searchElement, fromIndex) ──
    let typedarray_proto_last_index_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         search_element: i64,
         from_index: i64|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
                match ta_resolve(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_f64(-1.0),
                };
            let from_idx = if value::is_undefined(from_index) {
                (length as i32) - 1
            } else {
                let f = value::decode_f64(from_index);
                if f < 0.0 {
                    length as i32 + (f as i32).max(-1)
                } else {
                    (f as i32).min(length as i32 - 1)
                }
            };
            let end = if from_idx < 0 { 0 } else { from_idx as u32 + 1 };
            for i in (0..end).rev() {
                let elem = if is_shared {
                    sab_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                    )
                } else {
                    ta_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                    )
                }
                .unwrap_or(value::encode_undefined());
                if typedarray_same_value_zero(&mut caller, elem, search_element) {
                    return value::encode_f64(i as f64);
                }
            }
            value::encode_f64(-1.0)
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_last_index_of",
        typedarray_proto_last_index_of_fn,
    )?;

    // ── typedarray_proto_includes (Type 16, 3-arg: this, searchElement, fromIndex) ──
    let typedarray_proto_includes_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         search_element: i64,
         from_index: i64|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
                match ta_resolve(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_bool(false),
                };
            let from_idx = if value::is_undefined(from_index) {
                0i32
            } else {
                let f = value::decode_f64(from_index);
                if f < 0.0 {
                    length as i32 + (f as i32).max(0)
                } else {
                    (f as i32).min(length as i32)
                }
            };
            for i in from_idx as u32..length {
                let elem = if is_shared {
                    sab_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                    )
                } else {
                    ta_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                    )
                }
                .unwrap_or(value::encode_undefined());
                if typedarray_same_value_zero(&mut caller, elem, search_element) {
                    return value::encode_bool(true);
                }
            }
            value::encode_bool(false)
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_includes",
        typedarray_proto_includes_fn,
    )?;

    // ── typedarray_proto_join (Type 2, 2-arg: this, separator) ──
    let typedarray_proto_join_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, separator: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
                match ta_resolve(&mut caller, this_val) {
                    Some(v) => v,
                    None => return store_runtime_string(&caller, String::new()),
                };
            let sep = if value::is_undefined(separator) || value::is_null(separator) {
                ",".to_string()
            } else {
                get_string_value(&mut caller, separator)
            };
            let mut parts = Vec::new();
            for i in 0..length {
                let elem = if is_shared {
                    sab_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                    )
                } else {
                    ta_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                    )
                }
                .unwrap_or(value::encode_undefined());
                parts.push(render_value(&mut caller, elem).unwrap_or_else(|_| "".to_string()));
            }
            store_runtime_string(&caller, parts.join(&sep))
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_join",
        typedarray_proto_join_fn,
    )?;

    // ── typedarray_proto_to_string (Type 3, 1-arg: this) ──
    let typedarray_proto_to_string_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
                match ta_resolve(&mut caller, this_val) {
                    Some(v) => v,
                    None => return store_runtime_string(&caller, String::new()),
                };
            let mut parts = Vec::new();
            for i in 0..length {
                let elem = if is_shared {
                    sab_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                    )
                } else {
                    ta_read(
                        &mut caller,
                        buf_handle,
                        byte_offset,
                        elem_size,
                        element_kind,
                        i,
                    )
                }
                .unwrap_or(value::encode_undefined());
                parts.push(render_value(&mut caller, elem).unwrap_or_else(|_| "".to_string()));
            }
            store_runtime_string(&caller, parts.join(","))
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_to_string",
        typedarray_proto_to_string_fn,
    )?;

    // ── typedarray_proto_copy_within (Type 16, 3-arg: this, target, start, end via shadow stack) ──
    // Note: backend passes 3 WASM args (this, target, start) but end comes via shadow stack
    let typedarray_proto_copy_within_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         target_val: i64,
         start_val: i64,
         end_val: i64|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
                match ta_resolve(&mut caller, this_val) {
                    Some(v) => v,
                    None => return this_val,
                };
            let target = if value::is_undefined(target_val) {
                0
            } else {
                let f = value::decode_f64(target_val);
                if f < 0.0 {
                    (length as i32 + (f as i32)).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            let start = if value::is_undefined(start_val) {
                0
            } else {
                let f = value::decode_f64(start_val);
                if f < 0.0 {
                    (length as i32 + (f as i32)).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            let end = if value::is_undefined(end_val) {
                length
            } else {
                let f = value::decode_f64(end_val);
                if f < 0.0 {
                    (length as i32 + (f as i32)).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            let count = end.saturating_sub(start);
            let count = count.min(length.saturating_sub(target));
            if count == 0 {
                return this_val;
            }
            if target < start {
                for i in 0..count {
                    let elem = if is_shared {
                        sab_read(
                            &mut caller,
                            buf_handle,
                            byte_offset,
                            elem_size,
                            element_kind,
                            start + i,
                        )
                    } else {
                        ta_read(
                            &mut caller,
                            buf_handle,
                            byte_offset,
                            elem_size,
                            element_kind,
                            start + i,
                        )
                    }
                    .unwrap_or(value::encode_undefined());
                    if is_shared {
                        sab_write(
                            &mut caller,
                            buf_handle,
                            byte_offset,
                            elem_size,
                            element_kind,
                            target + i,
                            elem,
                        )
                    } else {
                        ta_write(
                            &mut caller,
                            buf_handle,
                            byte_offset,
                            elem_size,
                            element_kind,
                            target + i,
                            elem,
                        )
                    };
                }
            } else {
                for i in (0..count).rev() {
                    let elem = if is_shared {
                        sab_read(
                            &mut caller,
                            buf_handle,
                            byte_offset,
                            elem_size,
                            element_kind,
                            start + i,
                        )
                    } else {
                        ta_read(
                            &mut caller,
                            buf_handle,
                            byte_offset,
                            elem_size,
                            element_kind,
                            start + i,
                        )
                    }
                    .unwrap_or(value::encode_undefined());
                    if is_shared {
                        sab_write(
                            &mut caller,
                            buf_handle,
                            byte_offset,
                            elem_size,
                            element_kind,
                            target + i,
                            elem,
                        )
                    } else {
                        ta_write(
                            &mut caller,
                            buf_handle,
                            byte_offset,
                            elem_size,
                            element_kind,
                            target + i,
                            elem,
                        )
                    };
                }
            }
            this_val
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_copy_within",
        typedarray_proto_copy_within_fn,
    )?;

    // ── typedarray_proto_at (Type 2, 2-arg: this, index) ──
    let typedarray_proto_at_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
                match ta_resolve(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let idx = {
                let f = value::decode_f64(index);
                if f.is_nan() {
                    return value::encode_undefined();
                }
                if f < 0.0 {
                    length as i32 + f as i32
                } else {
                    f as i32
                }
            };
            if idx < 0 || idx >= length as i32 {
                return value::encode_undefined();
            }
            if is_shared {
                sab_read(
                    &mut caller,
                    buf_handle,
                    byte_offset,
                    elem_size,
                    element_kind,
                    idx as u32,
                )
            } else {
                ta_read(
                    &mut caller,
                    buf_handle,
                    byte_offset,
                    elem_size,
                    element_kind,
                    idx as u32,
                )
            }
            .unwrap_or(value::encode_undefined())
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_at",
        typedarray_proto_at_fn,
    )?;

    // ── typedarray_proto_entries (Type 3, 1-arg: this) ──
    let typedarray_proto_entries_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let Some(entry) = typedarray_entry_from_value(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let length = entry.length;
            let handle;
            {
                let mut iter_table = caller.data().iterators.lock().expect("iterators mutex");
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

    // ── typedarray_proto_keys (Type 3, 1-arg: this) ──
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
                let mut iter_table = caller.data().iterators.lock().expect("iterators mutex");
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

    // ── typedarray_proto_values (Type 3, 1-arg: this) ──
    let typedarray_proto_values_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let Some(entry) = typedarray_entry_from_value(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let length = entry.length;
            let handle;
            {
                let mut iter_table = caller.data().iterators.lock().expect("iterators mutex");
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
