use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::exec_context_impl::WasmExecContext;
use crate::*;

pub(crate) fn define_collections_buffers(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    // ── Map / Set / WeakMap / WeakSet / ArrayBuffer / Date：builtins 算法 ──
    linker.func_wrap_async(
        "env",
        "map_constructor",
        |mut caller: Caller<'_, RuntimeState>, (arg,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::collections::map_constructor(&mut ctx, arg).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "set_constructor",
        |mut caller: Caller<'_, RuntimeState>, (arg,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::collections::set_constructor(&mut ctx, arg).await
            })
        },
    )?;

    macro_rules! wrap2 {
        ($name:expr, $f:path) => {{
            let f = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
                    let mut ctx = WasmExecContext::new(&mut caller);
                    $f(&mut ctx, a, b)
                },
            );
            linker.define(&mut store, "env", $name, f)?;
        }};
    }
    macro_rules! wrap3 {
        ($name:expr, $f:path) => {{
            let f = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64, c: i64| -> i64 {
                    let mut ctx = WasmExecContext::new(&mut caller);
                    $f(&mut ctx, a, b, c)
                },
            );
            linker.define(&mut store, "env", $name, f)?;
        }};
    }
    macro_rules! wrap1 {
        ($name:expr, $f:path) => {{
            let f = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, a: i64| -> i64 {
                    let mut ctx = WasmExecContext::new(&mut caller);
                    $f(&mut ctx, a)
                },
            );
            linker.define(&mut store, "env", $name, f)?;
        }};
    }

    wrap3!("map_proto_set", wjsm_builtins::collections::map_proto_set);
    wrap2!("map_proto_get", wjsm_builtins::collections::map_proto_get);
    wrap2!("set_proto_add", wjsm_builtins::collections::set_proto_add);
    wrap2!("map_set_has", wjsm_builtins::collections::map_set_has);
    wrap2!("map_set_delete", wjsm_builtins::collections::map_set_delete);
    wrap1!("map_set_clear", wjsm_builtins::collections::map_set_clear);
    wrap1!("map_set_get_size", wjsm_builtins::collections::map_set_get_size);
    linker.func_wrap_async(
        "env",
        "map_set_for_each",
        |mut caller: Caller<'_, RuntimeState>,
         (_env, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::collections::map_set_for_each(
                    &mut ctx, this_val, args_base, args_count,
                )
                .await
            })
        },
    )?;
    wrap1!("map_set_keys", wjsm_builtins::collections::map_set_keys);
    wrap1!("map_set_values", wjsm_builtins::collections::map_set_values);
    wrap1!("map_set_entries", wjsm_builtins::collections::map_set_entries);

    let weakmap_ctor = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::collections::weakmap_constructor(&mut ctx)
        },
    );
    linker.define(&mut store, "env", "weakmap_constructor", weakmap_ctor)?;
    wrap3!("weakmap_proto_set", wjsm_builtins::collections::weakmap_proto_set);
    wrap2!("weakmap_proto_get", wjsm_builtins::collections::weakmap_proto_get);
    wrap2!("weakmap_proto_has", wjsm_builtins::collections::weakmap_proto_has);
    wrap2!(
        "weakmap_proto_delete",
        wjsm_builtins::collections::weakmap_proto_delete
    );

    let weakset_ctor = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::collections::weakset_constructor(&mut ctx)
        },
    );
    linker.define(&mut store, "env", "weakset_constructor", weakset_ctor)?;
    wrap2!("weakset_proto_add", wjsm_builtins::collections::weakset_proto_add);
    wrap2!("weakset_proto_has", wjsm_builtins::collections::weakset_proto_has);
    wrap2!(
        "weakset_proto_delete",
        wjsm_builtins::collections::weakset_proto_delete
    );

    wrap1!(
        "arraybuffer_constructor",
        wjsm_builtins::collections::arraybuffer_constructor
    );
    wrap1!(
        "arraybuffer_proto_byte_length",
        wjsm_builtins::collections::arraybuffer_proto_byte_length
    );
    wrap3!(
        "arraybuffer_proto_slice",
        wjsm_builtins::collections::arraybuffer_proto_slice
    );

    let dataview_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         buffer: i64,
         byte_offset: i64,
         byte_length: i64|
         -> i64 {
            let (buf_handle, buf_byte_length, is_shared) =
                match crate::shared_buffer::resolve_buffer_backing(&mut caller, buffer) {
                    Some(crate::shared_buffer::BufferBacking::SharedArrayBuffer {
                        handle,
                        byte_length,
                        ..
                    }) => (handle, byte_length, true),
                    Some(crate::shared_buffer::BufferBacking::ArrayBuffer {
                        handle,
                        byte_length,
                    }) => (handle, byte_length, false),
                    None => return value::encode_undefined(),
                };
            let offset = if value::is_undefined(byte_offset) {
                0
            } else {
                value::decode_f64(byte_offset) as u32
            };
            let length = if value::is_undefined(byte_length) {
                buf_byte_length.saturating_sub(offset)
            } else {
                value::decode_f64(byte_length) as u32
            };
            let handle;
            {
                let mut table = caller
                    .data()
                    .dataview_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                handle = table.len() as u32;
                table.push(DataViewEntry {
                    buffer_handle: buf_handle,
                    buffer_object: Some(buffer),
                    byte_offset: offset,
                    byte_length: length,
                    is_shared,
                });
            }
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 8)
            };
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__dataview_handle__",
                handle_val,
            );
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "dataview_constructor",
        dataview_constructor_fn,
    )?;

    macro_rules! dataview_get_fn {
        ($name:ident, $import:literal, $size:expr, $conv:expr) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, this_val: i64, byte_offset: i64| -> i64 {
                    let offset = value::decode_f64(byte_offset) as u32;
                    let obj_ptr = resolve_handle_idx(
                        &mut caller,
                        value::decode_object_handle(this_val) as usize,
                    );
                    let dv_handle = match obj_ptr {
                        Some(ptr) => {
                            match read_object_property_by_name(
                                &mut caller,
                                ptr,
                                "__dataview_handle__",
                            ) {
                                Some(v) => value::decode_f64(v) as usize,
                                None => return value::encode_undefined(),
                            }
                        }
                        None => return value::encode_undefined(),
                    };
                    let (buf_handle, dv_offset, dv_length, is_shared) = {
                        let dv_table = caller
                            .data()
                            .dataview_table
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        if dv_handle < dv_table.len() {
                            let e = &dv_table[dv_handle];
                            (e.buffer_handle, e.byte_offset, e.byte_length, e.is_shared)
                        } else {
                            return value::encode_undefined();
                        }
                    };
                    let abs_offset = dv_offset as usize + offset as usize;
                    if offset + $size as u32 > dv_length {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(
                            "RangeError: Offset is outside the bounds of the DataView".to_string(),
                        );
                        return value::encode_undefined();
                    }
                    let mut bytes = [0u8; 8];
                    if !crate::shared_buffer::dataview_read_bytes(
                        &mut caller,
                        buf_handle,
                        is_shared,
                        abs_offset,
                        &mut bytes[..$size],
                    ) {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(
                            "RangeError: Offset is outside the bounds of the DataView".to_string(),
                        );
                        return value::encode_undefined();
                    }
                    return $conv(&bytes[..$size]);
                },
            );
            linker.define(&mut store, "env", $import, $name)?;
        };
    }

    dataview_get_fn!(
        dataview_proto_get_int8_fn,
        "dataview_proto_get_int8",
        1,
        |bytes: &[u8]| value::encode_f64(bytes[0] as i8 as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_uint8_fn,
        "dataview_proto_get_uint8",
        1,
        |bytes: &[u8]| value::encode_f64(bytes[0] as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_int16_fn,
        "dataview_proto_get_int16",
        2,
        |bytes: &[u8]| value::encode_f64(i16::from_le_bytes([bytes[0], bytes[1]]) as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_uint16_fn,
        "dataview_proto_get_uint16",
        2,
        |bytes: &[u8]| value::encode_f64(u16::from_le_bytes([bytes[0], bytes[1]]) as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_int32_fn,
        "dataview_proto_get_int32",
        4,
        |bytes: &[u8]| value::encode_f64(i32::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3]
        ]) as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_uint32_fn,
        "dataview_proto_get_uint32",
        4,
        |bytes: &[u8]| value::encode_f64(u32::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3]
        ]) as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_float32_fn,
        "dataview_proto_get_float32",
        4,
        |bytes: &[u8]| value::encode_f64(f32::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3]
        ]) as f64)
    );
    dataview_get_fn!(
        dataview_proto_get_float64_fn,
        "dataview_proto_get_float64",
        8,
        |bytes: &[u8]| f64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]
        ])
        .to_bits() as i64
    );

    macro_rules! dataview_set_fn {
        ($name:ident, $import:literal, $size:expr, $write:expr) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>,
                 this_val: i64,
                 byte_offset: i64,
                 value_arg: i64|
                 -> i64 {
                    let offset = value::decode_f64(byte_offset) as u32;
                    let obj_ptr = resolve_handle_idx(
                        &mut caller,
                        value::decode_object_handle(this_val) as usize,
                    );
                    let dv_handle = match obj_ptr {
                        Some(ptr) => {
                            match read_object_property_by_name(
                                &mut caller,
                                ptr,
                                "__dataview_handle__",
                            ) {
                                Some(v) => value::decode_f64(v) as usize,
                                None => return value::encode_undefined(),
                            }
                        }
                        None => return value::encode_undefined(),
                    };
                    let (buf_handle, dv_offset, dv_length, is_shared) = {
                        let dv_table = caller
                            .data()
                            .dataview_table
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        if dv_handle < dv_table.len() {
                            let e = &dv_table[dv_handle];
                            (e.buffer_handle, e.byte_offset, e.byte_length, e.is_shared)
                        } else {
                            return value::encode_undefined();
                        }
                    };
                    let abs_offset = dv_offset as usize + offset as usize;
                    if offset + $size as u32 > dv_length {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(
                            "RangeError: Offset is outside the bounds of the DataView".to_string(),
                        );
                        return value::encode_undefined();
                    }
                    let bytes = $write(value_arg);
                    if !crate::shared_buffer::dataview_set_bytes(
                        &mut caller,
                        buf_handle,
                        is_shared,
                        abs_offset,
                        &bytes[..$size],
                    ) {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(
                            "RangeError: Offset is outside the bounds of the DataView".to_string(),
                        );
                        return value::encode_undefined();
                    }
                    value::encode_undefined()
                },
            );
            linker.define(&mut store, "env", $import, $name)?;
        };
    }

    dataview_set_fn!(
        dataview_proto_set_int8_fn,
        "dataview_proto_set_int8",
        1,
        |v: i64| (value::decode_f64(v) as i8).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_uint8_fn,
        "dataview_proto_set_uint8",
        1,
        |v: i64| (value::decode_f64(v) as u8).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_int16_fn,
        "dataview_proto_set_int16",
        2,
        |v: i64| (value::decode_f64(v) as i16).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_uint16_fn,
        "dataview_proto_set_uint16",
        2,
        |v: i64| (value::decode_f64(v) as u16).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_int32_fn,
        "dataview_proto_set_int32",
        4,
        |v: i64| (value::decode_f64(v) as i32).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_uint32_fn,
        "dataview_proto_set_uint32",
        4,
        |v: i64| (value::decode_f64(v) as u32).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_float32_fn,
        "dataview_proto_set_float32",
        4,
        |v: i64| (value::decode_f64(v) as f32).to_le_bytes().to_vec()
    );
    dataview_set_fn!(
        dataview_proto_set_float64_fn,
        "dataview_proto_set_float64",
        8,
        |v: i64| value::decode_f64(v).to_le_bytes().to_vec()
    );

    macro_rules! typedarray_constructor {
        ($name:ident, $import:literal, $size:expr, $kind:expr) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>,
                 buffer: i64,
                 byte_offset: i64,
                 length: i64|
                 -> i64 {
                    typedarray_construct(
                        &mut caller,
                        buffer,
                        byte_offset,
                        length,
                        $size,
                        $kind,
                        None,
                    )
                },
            );
            linker.define(&mut store, "env", $import, $name)?;
        };
    }

    typedarray_constructor!(int8array_constructor_fn, "int8array_constructor", 1, 0);
    typedarray_constructor!(uint8array_constructor_fn, "uint8array_constructor", 1, 1);
    typedarray_constructor!(
        uint8clampedarray_constructor_fn,
        "uint8clampedarray_constructor",
        1,
        2
    );
    typedarray_constructor!(int16array_constructor_fn, "int16array_constructor", 2, 0);
    typedarray_constructor!(uint16array_constructor_fn, "uint16array_constructor", 2, 1);
    typedarray_constructor!(int32array_constructor_fn, "int32array_constructor", 4, 0);
    typedarray_constructor!(uint32array_constructor_fn, "uint32array_constructor", 4, 1);
    typedarray_constructor!(
        float32array_constructor_fn,
        "float32array_constructor",
        4,
        3
    );
    typedarray_constructor!(
        float64array_constructor_fn,
        "float64array_constructor",
        8,
        3
    );
    typedarray_constructor!(
        bigint64array_constructor_fn,
        "bigint64array_constructor",
        8,
        4
    );
    typedarray_constructor!(
        biguint64array_constructor_fn,
        "biguint64array_constructor",
        8,
        5
    );
    let typedarray_proto_length_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => match read_object_property_by_name(&mut caller, ptr, "length") {
                    Some(v) => v,
                    None => value::encode_f64(0.0),
                },
                None => value::encode_f64(0.0),
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_length",
        typedarray_proto_length_fn,
    )?;

    let typedarray_proto_byte_length_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => match read_object_property_by_name(&mut caller, ptr, "byteLength") {
                    Some(v) => v,
                    None => value::encode_f64(0.0),
                },
                None => value::encode_f64(0.0),
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_byte_length",
        typedarray_proto_byte_length_fn,
    )?;

    let typedarray_proto_byte_offset_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => match read_object_property_by_name(&mut caller, ptr, "byteOffset") {
                    Some(v) => v,
                    None => value::encode_f64(0.0),
                },
                None => value::encode_f64(0.0),
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_byte_offset",
        typedarray_proto_byte_offset_fn,
    )?;

    let typedarray_proto_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         source: i64,
         offset_val: i64|
         -> i64 {
            let Some(target_entry) = typedarray_entry_from_value(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let offset = if value::is_undefined(offset_val) {
                0u32
            } else {
                value::decode_f64(offset_val) as u32
            };
            if offset > target_entry.length {
                return value::encode_undefined();
            }

            // 先收集源值，保证同一底层缓冲区重叠复制时不会边读边覆盖。
            let values: Vec<i64> = if value::is_array(source) {
                let Some(arr_ptr) = resolve_array_ptr(&mut caller, source) else {
                    return value::encode_undefined();
                };
                let src_length = read_array_length(&mut caller, arr_ptr).unwrap_or(0);
                if offset + src_length > target_entry.length {
                    return value::encode_undefined();
                }
                let mut values = Vec::with_capacity(src_length as usize);
                for i in 0..src_length {
                    values.push(
                        read_array_elem(&mut caller, arr_ptr, i)
                            .unwrap_or_else(value::encode_undefined),
                    );
                }
                values
            } else if let Some(src_entry) = typedarray_entry_from_value(&mut caller, source) {
                if offset + src_entry.length > target_entry.length {
                    return value::encode_undefined();
                }
                let mut values = Vec::with_capacity(src_entry.length as usize);
                for i in 0..src_entry.length {
                    values.push(
                        typedarray_element_read(&mut caller, source, i)
                            .unwrap_or_else(value::encode_undefined),
                    );
                }
                values
            } else {
                return value::encode_undefined();
            };

            for (i, value) in values.into_iter().enumerate() {
                let _ = typedarray_element_write(&mut caller, this_val, offset + i as u32, value);
            }
            value::encode_undefined()
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_set",
        typedarray_proto_set_fn,
    )?;

    let typedarray_proto_slice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         begin_val: i64,
         end_val: i64|
         -> i64 {
            // Resolve TypedArray
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let Some(ptr) =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize)
            else {
                return value::encode_undefined();
            };
            let Some(h) = read_object_property_by_name(&mut caller, ptr, "__typedarray_handle__")
            else {
                return value::encode_undefined();
            };
            let handle = value::decode_f64(h) as usize;
            let ta_table = caller
                .data()
                .typedarray_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(entry) = ta_table.get(handle) else {
                return value::encode_undefined();
            };
            let buf_handle = entry.buffer_handle;
            let byte_offset = entry.byte_offset;
            let length = entry.length;
            let elem_size = entry.element_size;
            let element_kind = entry.element_kind;
            let is_shared = entry.is_shared;
            drop(ta_table);

            // Clamp begin
            let begin = if value::is_undefined(begin_val) {
                0u32
            } else {
                let f = value::decode_f64(begin_val);
                if f < 0.0 {
                    (length as i32 + f as i32).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            // Clamp end
            let end = if value::is_undefined(end_val) {
                length
            } else {
                let f = value::decode_f64(end_val);
                if f < 0.0 {
                    (length as i32 + f as i32).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            let slice_len = end.saturating_sub(begin);
            if slice_len == 0 {
                // Create empty TypedArray
                let new_buf_handle;
                {
                    let mut ab_table = caller
                        .data()
                        .arraybuffer_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    new_buf_handle = ab_table.len() as u32;
                    ab_table.push(ArrayBufferEntry { data: Vec::new() });
                }
                let new_ta_handle;
                {
                    let mut ta_table = caller
                        .data()
                        .typedarray_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    new_ta_handle = ta_table.len() as u32;
                    ta_table.push(TypedArrayEntry {
                        buffer_handle: new_buf_handle,
                        buffer_object: None,
                        byte_offset: 0,
                        length: 0,
                        element_size: elem_size,
                        element_kind,
                        is_shared: false,
                    });
                }
                let obj = {
                    let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                    alloc_host_object(&mut caller, &_wjsm_env, 8)
                };
                let _ = define_host_data_property_from_caller(
                    &mut caller,
                    obj,
                    "__typedarray_handle__",
                    value::encode_f64(new_ta_handle as f64),
                );
                let _ = define_host_data_property_from_caller(
                    &mut caller,
                    obj,
                    "length",
                    value::encode_f64(0.0),
                );
                let _ = define_host_data_property_from_caller(
                    &mut caller,
                    obj,
                    "byteLength",
                    value::encode_f64(0.0),
                );
                let _ = define_host_data_property_from_caller(
                    &mut caller,
                    obj,
                    "byteOffset",
                    value::encode_f64(0.0),
                );
                return obj;
            }

            // Create new ArrayBuffer with sliced bytes
            let src_byte_start = byte_offset as usize + (begin as usize) * (elem_size as usize);
            let slice_byte_len = slice_len as usize * elem_size as usize;
            let sliced_data: Vec<u8> = if is_shared {
                let shared = caller
                    .data()
                    .shared_state
                    .clone()
                    .expect("SharedArrayBuffer requires shared_state");
                let sab_table = shared.sab_table.lock().unwrap_or_else(|e| e.into_inner());
                let Some(buf_entry) = sab_table.get(buf_handle as usize) else {
                    return value::encode_undefined();
                };
                let guard = buf_entry.data.read().expect("sab read lock");
                let end_off = src_byte_start + slice_byte_len;
                if end_off > guard.len() {
                    return value::encode_undefined();
                }
                let data = guard[src_byte_start..end_off].to_vec();
                drop(guard);
                drop(sab_table);
                data
            } else {
                let ab_table = caller
                    .data()
                    .arraybuffer_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let Some(buf_entry) = ab_table.get(buf_handle as usize) else {
                    return value::encode_undefined();
                };
                let end_off = src_byte_start + slice_byte_len;
                if end_off > buf_entry.data.len() {
                    return value::encode_undefined();
                }
                let data = buf_entry.data[src_byte_start..end_off].to_vec();
                drop(ab_table);
                data
            };
            let new_buf_handle;
            {
                let mut ab_table = caller
                    .data()
                    .arraybuffer_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                new_buf_handle = ab_table.len() as u32;
                ab_table.push(ArrayBufferEntry { data: sliced_data });
            }

            // Create new TypedArray entry pointing to the new buffer
            let new_ta_handle;
            {
                let mut ta_table = caller
                    .data()
                    .typedarray_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                new_ta_handle = ta_table.len() as u32;
                ta_table.push(TypedArrayEntry {
                    buffer_handle: new_buf_handle,
                    buffer_object: None,
                    byte_offset: 0,
                    length: slice_len,
                    element_size: elem_size,
                    element_kind,
                    is_shared: false,
                });
            }

            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 8)
            };
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__typedarray_handle__",
                value::encode_f64(new_ta_handle as f64),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "length",
                value::encode_f64(slice_len as f64),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "byteLength",
                value::encode_f64((slice_len * elem_size as u32) as f64),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "byteOffset",
                value::encode_f64(0.0),
            );
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_slice",
        typedarray_proto_slice_fn,
    )?;

    let typedarray_proto_subarray_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         begin_val: i64,
         end_val: i64|
         -> i64 {
            // Resolve TypedArray
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let Some(ptr) =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize)
            else {
                return value::encode_undefined();
            };
            let Some(h) = read_object_property_by_name(&mut caller, ptr, "__typedarray_handle__")
            else {
                return value::encode_undefined();
            };
            let handle = value::decode_f64(h) as usize;
            let ta_table = caller
                .data()
                .typedarray_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(entry) = ta_table.get(handle) else {
                return value::encode_undefined();
            };
            let buf_handle = entry.buffer_handle;
            let buffer_object = entry.buffer_object;
            let byte_offset = entry.byte_offset;
            let length = entry.length;
            let elem_size = entry.element_size;
            let element_kind = entry.element_kind;
            let sub_is_shared = entry.is_shared;
            drop(ta_table);

            // Clamp begin
            let begin = if value::is_undefined(begin_val) {
                0u32
            } else {
                let f = value::decode_f64(begin_val);
                if f < 0.0 {
                    (length as i32 + f as i32).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            // Clamp end
            let end = if value::is_undefined(end_val) {
                length
            } else {
                let f = value::decode_f64(end_val);
                if f < 0.0 {
                    (length as i32 + f as i32).max(0) as u32
                } else {
                    (f as u32).min(length)
                }
            };
            let sub_len = end.saturating_sub(begin);
            let new_byte_offset = byte_offset + begin * elem_size as u32;

            // Create new TypedArray entry sharing the same ArrayBuffer
            let new_ta_handle;
            {
                let mut ta_table = caller
                    .data()
                    .typedarray_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                new_ta_handle = ta_table.len() as u32;
                ta_table.push(TypedArrayEntry {
                    buffer_handle: buf_handle,
                    buffer_object,
                    byte_offset: new_byte_offset,
                    length: sub_len,
                    element_size: elem_size,
                    element_kind,
                    is_shared: sub_is_shared,
                });
            }

            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 8)
            };
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__typedarray_handle__",
                value::encode_f64(new_ta_handle as f64),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "length",
                value::encode_f64(sub_len as f64),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "byteLength",
                value::encode_f64((sub_len * elem_size as u32) as f64),
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "byteOffset",
                value::encode_f64(new_byte_offset as f64),
            );
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_subarray",
        typedarray_proto_subarray_fn,
    )?;

    let create_global_object_fn =
        Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>| -> i64 {
            // 单例：嵌套函数入口会再次调用 create_global_object 填充 `$0.$global` local。
            // 若每次新建，globalThis 上的 JS 属性（如 `__wjsm_cluster`）在函数间不可见。
            let existing = caller
                .data()
                .js_global_object
                .load(std::sync::atomic::Ordering::Relaxed);
            if !value::is_undefined(existing) {
                return existing;
            }
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 128)
            };
            let temp_root_len = caller.data().push_host_temp_roots([obj]);
            let builtin_pairs: &[(&str, NativeCallable)] = &[
                ("Array", NativeCallable::ArrayConstructor),
                ("Object", NativeCallable::ObjectConstructor),
                ("Function", NativeCallable::FunctionConstructor),
                ("String", NativeCallable::StringConstructor),
                ("Boolean", NativeCallable::BooleanConstructor),
                ("Number", NativeCallable::NumberConstructor),
                ("Symbol", NativeCallable::SymbolConstructor),
                ("BigInt", NativeCallable::BigIntConstructor),
                ("RegExp", NativeCallable::RegExpConstructor),
                ("Error", NativeCallable::ErrorConstructor),
                ("TypeError", NativeCallable::TypeErrorConstructor),
                ("RangeError", NativeCallable::RangeErrorConstructor),
                ("SyntaxError", NativeCallable::SyntaxErrorConstructor),
                ("ReferenceError", NativeCallable::ReferenceErrorConstructor),
                ("URIError", NativeCallable::URIErrorConstructor),
                ("EvalError", NativeCallable::EvalErrorConstructor),
                ("AggregateError", NativeCallable::AggregateErrorConstructor),
                ("Map", NativeCallable::MapConstructor),
                ("Set", NativeCallable::SetConstructor),
                ("WeakMap", NativeCallable::WeakMapConstructor),
                ("WeakSet", NativeCallable::WeakSetConstructor),
                ("WeakRef", NativeCallable::WeakRefConstructor),
                (
                    "FinalizationRegistry",
                    NativeCallable::FinalizationRegistryConstructor,
                ),
                ("Date", NativeCallable::DateConstructorGlobal),
                ("Promise", NativeCallable::PromiseConstructor),
                ("Headers", NativeCallable::HeadersConstructor),
                ("Request", NativeCallable::RequestConstructor),
                ("Response", NativeCallable::ResponseConstructor),
                ("ReadableStream", NativeCallable::ReadableStreamConstructor),
                ("WritableStream", NativeCallable::WritableStreamConstructor),
                (
                    "TransformStream",
                    NativeCallable::TransformStreamConstructor,
                ),
                (
                    "CountQueuingStrategy",
                    NativeCallable::CountQueuingStrategyConstructor,
                ),
                (
                    "ByteLengthQueuingStrategy",
                    NativeCallable::ByteLengthQueuingStrategyConstructor,
                ),
                (
                    "AbortController",
                    NativeCallable::AbortControllerConstructor,
                ),
                ("ArrayBuffer", NativeCallable::ArrayBufferConstructorGlobal),
                (
                    "SharedArrayBuffer",
                    NativeCallable::SharedArrayBufferConstructor,
                ),
                ("Atomics", NativeCallable::AtomicsGlobal),
                ("DataView", NativeCallable::DataViewConstructorGlobal),
                (
                    "Int8Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Int8),
                ),
                (
                    "Uint8Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint8),
                ),
                (
                    "Uint8ClampedArray",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint8Clamped),
                ),
                (
                    "Int16Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Int16),
                ),
                (
                    "Uint16Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint16),
                ),
                (
                    "Int32Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Int32),
                ),
                (
                    "Uint32Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint32),
                ),
                (
                    "Float32Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Float32),
                ),
                (
                    "Float64Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Float64),
                ),
                (
                    "BigInt64Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::BigInt64),
                ),
                (
                    "BigUint64Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::BigUint64),
                ),
                ("Proxy", NativeCallable::ProxyConstructor),
                ("gc", NativeCallable::GcCollect),
            ];

            for (name, callable) in builtin_pairs {
                let mut native_callables = caller
                    .data()
                    .native_callables
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let idx = native_callables.len() as u32;
                native_callables.push(callable.clone());
                let val = value::encode_native_callable_idx(idx);
                drop(native_callables);
                let _ = define_host_data_property_from_caller(&mut caller, obj, name, val);
                if *name == "Symbol" {
                    crate::symbol_well_known::install_well_known_symbols_on_symbol_constructor(
                        &mut caller,
                        val,
                    );
                }
            }

            let _ = define_host_data_property_from_caller(&mut caller, obj, "globalThis", obj);
            let _ = install_process_global_from_caller(&mut caller, obj);
            let _ =
                crate::runtime_node_globals::install_node_web_globals_from_caller(&mut caller, obj);

            // test262 harness: global `$262` with `.agent` methods
            let agent_obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 7)
            };
            let _ = caller.data().push_host_temp_roots([agent_obj]);
            let agent_methods: &[(&str, NativeCallable)] = &[
                ("start", NativeCallable::AgentStart),
                ("broadcast", NativeCallable::AgentBroadcast),
                ("receiveBroadcast", NativeCallable::AgentReceiveBroadcast),
                ("getReport", NativeCallable::AgentGetReport),
                ("report", NativeCallable::AgentReport),
                ("sleep", NativeCallable::AgentSleep),
                ("monotonicNow", NativeCallable::AgentMonotonicNow),
            ];
            for (name, callable) in agent_methods {
                let mut nc = caller
                    .data()
                    .native_callables
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let idx = nc.len() as u32;
                nc.push(callable.clone());
                let val = value::encode_native_callable_idx(idx);
                drop(nc);
                let _ = define_host_data_property_from_caller(&mut caller, agent_obj, name, val);
            }
            let harness_obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 1)
            };
            let _ = caller.data().push_host_temp_roots([harness_obj]);
            let _ =
                define_host_data_property_from_caller(&mut caller, harness_obj, "agent", agent_obj);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "$262", harness_obj);

            caller.data().truncate_host_temp_roots(temp_root_len);
            // 永久 root：事件循环回调之间 main local 可能不在栈上。
            caller
                .data()
                .js_global_object
                .store(obj, std::sync::atomic::Ordering::Relaxed);
            obj
        });
    linker.define(
        &mut store,
        "env",
        "create_global_object",
        create_global_object_fn,
    )?;

    let create_exception_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, thrown_value: i64| -> i64 {
            let rendered =
                render_value(&mut caller, thrown_value).unwrap_or_else(|_| "unknown".to_string());
            let mut errors = caller
                .data()
                .error_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let idx = errors.len() as u32;
            errors.push(ErrorEntry {
                name: String::new(),
                message: rendered,
                value: thrown_value,
            });
            value::encode_handle(value::TAG_EXCEPTION, idx)
        },
    );
    linker.define(&mut store, "env", "create_exception", create_exception_fn)?;

    let exception_value_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, exception_handle: i64| -> i64 {
            let idx = value::decode_handle(exception_handle) as usize;
            let errors = caller
                .data()
                .error_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            errors
                .get(idx)
                .map(|e| e.value)
                .unwrap_or(value::encode_undefined())
        },
    );
    linker.define(&mut store, "env", "exception_value", exception_value_fn)?;


    // ── Date ──
    let date_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         _this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let args: Vec<i64> = if args_count > 0 {
                (0..args_count.max(0) as u32)
                    .map(|i| read_shadow_arg(&mut caller, args_base, i))
                    .collect()
            } else {
                vec![]
            };

            use std::sync::atomic::Ordering;

            let new_target = caller.data().new_target.load(Ordering::Relaxed);
            if value::is_undefined(new_target) {
                let now_ms = chrono::Utc::now().timestamp_millis() as f64;
                if now_ms.is_nan() {
                    return store_runtime_string(&caller, "Invalid Date".to_string());
                }
                return match ms_to_datetime_local(now_ms) {
                    Some(dt) => {
                        let s = dt.format("%a %b %e %Y %H:%M:%S GMT%:z").to_string();
                        store_runtime_string(&caller, s)
                    }
                    None => store_runtime_string(&caller, "Invalid Date".to_string()),
                };
            }

            let ms = if args.is_empty() {
                let now = chrono::Utc::now();
                now.timestamp_millis() as f64
            } else if args.len() == 1 {
                let arg = args[0];
                if value::is_undefined(arg) {
                    let now = chrono::Utc::now();
                    now.timestamp_millis() as f64
                } else if value::is_f64(arg) {
                    let val = value::decode_f64(arg);
                    if val.is_nan() || val.is_infinite() {
                        f64::NAN
                    } else {
                        val
                    }
                } else if value::is_string(arg) {
                    let s = read_value_string_bytes(&mut caller, arg)
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default();
                    parse_date_string(&s).unwrap_or(f64::NAN)
                } else if value::is_object(arg) {
                    read_date_ms(&mut caller, arg)
                } else {
                    f64::NAN
                }
            } else {
                date_args_to_ms(&args, false)
            };

            let state = caller.data();
            let (
                get_date_fn,
                get_day_fn,
                get_full_year_fn,
                get_hours_fn,
                get_milliseconds_fn,
                get_minutes_fn,
                get_month_fn,
                get_seconds_fn,
                get_time_fn,
                get_timezone_offset_fn,
                get_utc_date_fn,
                get_utc_day_fn,
                get_utc_full_year_fn,
                get_utc_hours_fn,
                get_utc_milliseconds_fn,
                get_utc_minutes_fn,
                get_utc_month_fn,
                get_utc_seconds_fn,
                set_date_fn,
                set_full_year_fn,
                set_hours_fn,
                set_milliseconds_fn,
                set_minutes_fn,
                set_month_fn,
                set_seconds_fn,
                set_time_fn,
                set_utc_date_fn,
                set_utc_full_year_fn,
                set_utc_hours_fn,
                set_utc_milliseconds_fn,
                set_utc_minutes_fn,
                set_utc_month_fn,
                set_utc_seconds_fn,
                to_string_fn,
                to_date_string_fn,
                to_time_string_fn,
                to_locale_string_fn,
                to_locale_date_string_fn,
                to_locale_time_string_fn,
                to_iso_string_fn,
                to_utc_string_fn,
                to_json_fn,
                value_of_fn,
            ) = {
                (
                    create_date_method(state, DateMethodKind::GetDate),
                    create_date_method(state, DateMethodKind::GetDay),
                    create_date_method(state, DateMethodKind::GetFullYear),
                    create_date_method(state, DateMethodKind::GetHours),
                    create_date_method(state, DateMethodKind::GetMilliseconds),
                    create_date_method(state, DateMethodKind::GetMinutes),
                    create_date_method(state, DateMethodKind::GetMonth),
                    create_date_method(state, DateMethodKind::GetSeconds),
                    create_date_method(state, DateMethodKind::GetTime),
                    create_date_method(state, DateMethodKind::GetTimezoneOffset),
                    create_date_method(state, DateMethodKind::GetUTCDate),
                    create_date_method(state, DateMethodKind::GetUTCDay),
                    create_date_method(state, DateMethodKind::GetUTCFullYear),
                    create_date_method(state, DateMethodKind::GetUTCHours),
                    create_date_method(state, DateMethodKind::GetUTCMilliseconds),
                    create_date_method(state, DateMethodKind::GetUTCMinutes),
                    create_date_method(state, DateMethodKind::GetUTCMonth),
                    create_date_method(state, DateMethodKind::GetUTCSeconds),
                    create_date_method(state, DateMethodKind::SetDate),
                    create_date_method(state, DateMethodKind::SetFullYear),
                    create_date_method(state, DateMethodKind::SetHours),
                    create_date_method(state, DateMethodKind::SetMilliseconds),
                    create_date_method(state, DateMethodKind::SetMinutes),
                    create_date_method(state, DateMethodKind::SetMonth),
                    create_date_method(state, DateMethodKind::SetSeconds),
                    create_date_method(state, DateMethodKind::SetTime),
                    create_date_method(state, DateMethodKind::SetUTCDate),
                    create_date_method(state, DateMethodKind::SetUTCFullYear),
                    create_date_method(state, DateMethodKind::SetUTCHours),
                    create_date_method(state, DateMethodKind::SetUTCMilliseconds),
                    create_date_method(state, DateMethodKind::SetUTCMinutes),
                    create_date_method(state, DateMethodKind::SetUTCMonth),
                    create_date_method(state, DateMethodKind::SetUTCSeconds),
                    create_date_method(state, DateMethodKind::ToString),
                    create_date_method(state, DateMethodKind::ToDateString),
                    create_date_method(state, DateMethodKind::ToTimeString),
                    create_date_method(state, DateMethodKind::ToLocaleString),
                    create_date_method(state, DateMethodKind::ToLocaleDateString),
                    create_date_method(state, DateMethodKind::ToLocaleTimeString),
                    create_date_method(state, DateMethodKind::ToISOString),
                    create_date_method(state, DateMethodKind::ToUTCString),
                    create_date_method(state, DateMethodKind::ToJSON),
                    create_date_method(state, DateMethodKind::ValueOf),
                )
            };

            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                let obj = alloc_host_object(&mut caller, &_wjsm_env, 43);
                if let Some(proto) = native_callable_date_prototype(
                    &mut caller,
                    &NativeCallable::DateConstructorGlobal,
                ) {
                    set_object_proto_header(&mut caller, &_wjsm_env, obj, proto);
                }
                obj
            };
            let ms_val = value::encode_f64(ms);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__date_ms__", ms_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getDate", get_date_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getDay", get_day_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getFullYear",
                get_full_year_fn,
            );
            let _ =
                define_host_data_property_from_caller(&mut caller, obj, "getHours", get_hours_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getMilliseconds",
                get_milliseconds_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getMinutes",
                get_minutes_fn,
            );
            let _ =
                define_host_data_property_from_caller(&mut caller, obj, "getMonth", get_month_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getSeconds",
                get_seconds_fn,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getTime", get_time_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getTimezoneOffset",
                get_timezone_offset_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCDate",
                get_utc_date_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCDay",
                get_utc_day_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCFullYear",
                get_utc_full_year_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCHours",
                get_utc_hours_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCMilliseconds",
                get_utc_milliseconds_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCMinutes",
                get_utc_minutes_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCMonth",
                get_utc_month_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "getUTCSeconds",
                get_utc_seconds_fn,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setDate", set_date_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setFullYear",
                set_full_year_fn,
            );
            let _ =
                define_host_data_property_from_caller(&mut caller, obj, "setHours", set_hours_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setMilliseconds",
                set_milliseconds_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setMinutes",
                set_minutes_fn,
            );
            let _ =
                define_host_data_property_from_caller(&mut caller, obj, "setMonth", set_month_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setSeconds",
                set_seconds_fn,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setTime", set_time_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCDate",
                set_utc_date_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCFullYear",
                set_utc_full_year_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCHours",
                set_utc_hours_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCMilliseconds",
                set_utc_milliseconds_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCMinutes",
                set_utc_minutes_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCMonth",
                set_utc_month_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "setUTCSeconds",
                set_utc_seconds_fn,
            );
            let _ =
                define_host_data_property_from_caller(&mut caller, obj, "toString", to_string_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "toDateString",
                to_date_string_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "toTimeString",
                to_time_string_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "toLocaleString",
                to_locale_string_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "toLocaleDateString",
                to_locale_date_string_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "toLocaleTimeString",
                to_locale_time_string_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "toISOString",
                to_iso_string_fn,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "toUTCString",
                to_utc_string_fn,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toJSON", to_json_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "valueOf", value_of_fn);
            obj
        },
    );
    linker.define(&mut store, "env", "date_constructor", date_constructor_fn)?;


    let date_now_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>| -> i64 {
        let now = chrono::Utc::now();
        value::encode_f64(now.timestamp_millis() as f64)
    });
    linker.define(&mut store, "env", "date_now", date_now_fn)?;

    let date_parse_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let s = if value::is_string(arg) {
                read_value_string_bytes(&mut caller, arg)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            if s.is_empty() {
                return value::encode_f64(f64::NAN);
            }
            value::encode_f64(parse_date_string(&s).unwrap_or(f64::NAN))
        },
    );
    linker.define(&mut store, "env", "date_parse", date_parse_fn)?;

    let date_utc_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
            let args: Vec<i64> = if args_count > 0 {
                (0..args_count.max(0) as u32)
                    .map(|i| read_shadow_arg(&mut caller, args_base, i))
                    .collect()
            } else {
                vec![]
            };
            let ms = date_args_to_ms(&args, true);
            value::encode_f64(ms)
        },
    );
    linker.define(&mut store, "env", "date_utc", date_utc_fn)?;

    super::private_fields::define_private_fields(linker, store)?;
    Ok(())
}
