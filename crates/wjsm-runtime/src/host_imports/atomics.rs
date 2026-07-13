use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::*;

pub(crate) fn define_atomics(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    /// 统一 Atomics 访问准备：ToIndex、length OOB RangeError、整数 TA 检查 TypeError（float/clamped 拒绝）。
    /// 返回 AccessCtx；对非 wait RMW 允许 AB 上的整数 TA（is_shared=false），wait 路径单独要求 shared。
    /// buffer_handle 指向 arraybuffer_table 或 sab_table（由 is_shared 决定）。
    fn prepare_atomics_access(
        caller: &mut Caller<'_, RuntimeState>,
        this_val: i64,
        index_val: i64,
    ) -> Option<(usize, u8, u8, usize, bool)> {
        let entry = match typedarray_entry_from_value(caller, this_val) {
            Some(e) if e.element_kind != 2 && e.element_kind != 3 => e,
            _ => {
                set_typedarray_runtime_error(
                    caller,
                    "TypeError: Typed array is not an integer type for Atomics",
                );
                return None;
            }
        };
        let index =
            typedarray_to_index(caller, index_val, "RangeError: Invalid typed array index")?;
        if index >= entry.length {
            set_typedarray_runtime_error(caller, "RangeError: Invalid typed array index");
            return None;
        }
        let byte_offset =
            entry.byte_offset as usize + (index as usize) * (entry.element_size as usize);
        let is_shared = entry.is_shared;
        let buf_handle = entry.buffer_handle as usize;
        Some((
            byte_offset,
            entry.element_size,
            entry.element_kind,
            buf_handle,
            is_shared,
        ))
    }
    /// wait/waitAsync/notify 必须在 shared TA 上；否则 TypeError。
    fn prepare_waitable_access(
        caller: &mut Caller<'_, RuntimeState>,
        this_val: i64,
        index_val: i64,
    ) -> Option<(usize, u8, u8, usize)> {
        let (byte_offset, elem_size, element_kind, buf_handle, is_shared) =
            prepare_atomics_access(caller, this_val, index_val)?;
        if !is_shared {
            set_runtime_error(
                caller.data(),
                "TypeError: wait/notify/waitAsync called on non-shared TypedArray".to_string(),
            );
            return None;
        }
        Some((byte_offset, elem_size, element_kind, buf_handle))
    }

    /// 对 buffer 加锁执行字节级 RMW，返回旧值编码（Number 用 f64 encode，BigInt 用 handle）。
    /// 支持 SAB (is_shared, sab_table + inner RwLock) 和 AB-backed (!is_shared, arraybuffer_table + Vec 直接 mut under table lock)。
    #[allow(clippy::too_many_arguments)]
    fn atomic_rmw(
        caller: &mut Caller<'_, RuntimeState>,
        buffer_handle: usize,
        off: usize,
        elem_size: u8,
        element_kind: u8,
        value_raw: i64,
        is_shared: bool,
        op: fn(i64, i64) -> i64,
    ) -> Option<i64> {
        if is_shared {
            let shared = caller.data().shared_state.as_ref()?.clone();
            let table = shared.sab_table.lock().ok()?;
            let entry = table.get(buffer_handle)?;
            let mut data = entry.data.write().ok()?;
            if off + elem_size as usize > data.len() {
                return None;
            }
            let old = match (elem_size, element_kind) {
                (1, 0) => {
                    let v = (value::decode_f64(value_raw) as i8) as i64;
                    let o = data[off] as i8 as i64;
                    data[off] = op(o, v) as u8;
                    value::encode_f64(o as f64)
                }
                (1, 1) => {
                    let v = (value::decode_f64(value_raw) as u8) as i64;
                    let o = data[off] as i64;
                    data[off] = op(o, v) as u8;
                    value::encode_f64(o as f64)
                }
                (2, 0) => {
                    let v = (value::decode_f64(value_raw) as i16) as i64;
                    let o = i16::from_le_bytes([data[off], data[off + 1]]) as i64;
                    let r = op(o, v);
                    let bytes = (r as i16).to_le_bytes();
                    data[off..off + 2].copy_from_slice(&bytes);
                    value::encode_f64(o as f64)
                }
                (2, 1) => {
                    let v = (value::decode_f64(value_raw) as u16) as i64;
                    let o = u16::from_le_bytes([data[off], data[off + 1]]) as i64;
                    let r = op(o, v);
                    let bytes = (r as u16).to_le_bytes();
                    data[off..off + 2].copy_from_slice(&bytes);
                    value::encode_f64(o as f64)
                }
                (4, 0) => {
                    let v = (value::decode_f64(value_raw) as i32) as i64;
                    let o = i32::from_le_bytes([
                        data[off],
                        data[off + 1],
                        data[off + 2],
                        data[off + 3],
                    ]) as i64;
                    let r = op(o, v);
                    let bytes = (r as i32).to_le_bytes();
                    data[off..off + 4].copy_from_slice(&bytes);
                    value::encode_f64(o as f64)
                }
                (4, 1) => {
                    let v = (value::decode_f64(value_raw) as u32) as i64;
                    let o = u32::from_le_bytes([
                        data[off],
                        data[off + 1],
                        data[off + 2],
                        data[off + 3],
                    ]) as i64;
                    let r = op(o, v);
                    let bytes = (r as u32).to_le_bytes();
                    data[off..off + 4].copy_from_slice(&bytes);
                    value::encode_f64(o as f64)
                }
                (8, 4) => {
                    let v_handle = value::decode_bigint_handle(value_raw) as usize;
                    let bigint_table = caller.data().bigint_table.lock().ok()?;
                    let v_bi = bigint_table.get(v_handle)?.clone();
                    let v_bytes = bigint_low_64_bytes(&v_bi);
                    let v64 = i64::from_le_bytes(v_bytes);
                    let o64 = i64::from_le_bytes([
                        data[off],
                        data[off + 1],
                        data[off + 2],
                        data[off + 3],
                        data[off + 4],
                        data[off + 5],
                        data[off + 6],
                        data[off + 7],
                    ]);
                    let r64 = op(o64, v64);
                    drop(bigint_table);
                    let r_bytes = r64.to_le_bytes();
                    data[off..off + 8].copy_from_slice(&r_bytes);
                    let mut bigint_table = caller.data().bigint_table.lock().ok()?;
                    let handle = bigint_table.len() as u32;
                    bigint_table.push(num_bigint::BigInt::from(o64));
                    value::encode_bigint_handle(handle)
                }
                (8, 5) => {
                    let v_handle = value::decode_bigint_handle(value_raw) as usize;
                    let bigint_table = caller.data().bigint_table.lock().ok()?;
                    let v_bi = bigint_table.get(v_handle)?.clone();
                    let v_bytes = bigint_low_64_bytes(&v_bi);
                    let v64 = u64::from_le_bytes(v_bytes) as i64;
                    let o_bytes: [u8; 8] = [
                        data[off],
                        data[off + 1],
                        data[off + 2],
                        data[off + 3],
                        data[off + 4],
                        data[off + 5],
                        data[off + 6],
                        data[off + 7],
                    ];
                    let o64 = u64::from_le_bytes(o_bytes) as i64;
                    let r64 = op(o64, v64);
                    let r_bytes = (r64 as u64).to_le_bytes();
                    data[off..off + 8].copy_from_slice(&r_bytes);
                    let mut bigint_table = caller.data().bigint_table.lock().ok()?;
                    let handle = bigint_table.len() as u32;
                    bigint_table.push(num_bigint::BigInt::from(o64 as u64));
                    value::encode_bigint_handle(handle)
                }
                _ => return None,
            };
            Some(old)
        } else {
            // AB-backed: lock the ab_table for mutation duration (single-thread ok for atomic semantics)
            let mut ab_table = caller.data().arraybuffer_table.lock().ok()?;
            let ab_entry = ab_table.get_mut(buffer_handle)?;
            let data = &mut ab_entry.data;
            if off + elem_size as usize > data.len() {
                return None;
            }
            let old = match (elem_size, element_kind) {
                (1, 0) => {
                    let v = (value::decode_f64(value_raw) as i8) as i64;
                    let o = data[off] as i8 as i64;
                    data[off] = op(o, v) as u8;
                    value::encode_f64(o as f64)
                }
                (1, 1) => {
                    let v = (value::decode_f64(value_raw) as u8) as i64;
                    let o = data[off] as i64;
                    data[off] = op(o, v) as u8;
                    value::encode_f64(o as f64)
                }
                (2, 0) => {
                    let v = (value::decode_f64(value_raw) as i16) as i64;
                    let o = i16::from_le_bytes([data[off], data[off + 1]]) as i64;
                    let r = op(o, v);
                    let bytes = (r as i16).to_le_bytes();
                    data[off..off + 2].copy_from_slice(&bytes);
                    value::encode_f64(o as f64)
                }
                (2, 1) => {
                    let v = (value::decode_f64(value_raw) as u16) as i64;
                    let o = u16::from_le_bytes([data[off], data[off + 1]]) as i64;
                    let r = op(o, v);
                    let bytes = (r as u16).to_le_bytes();
                    data[off..off + 2].copy_from_slice(&bytes);
                    value::encode_f64(o as f64)
                }
                (4, 0) => {
                    let v = (value::decode_f64(value_raw) as i32) as i64;
                    let o = i32::from_le_bytes([
                        data[off],
                        data[off + 1],
                        data[off + 2],
                        data[off + 3],
                    ]) as i64;
                    let r = op(o, v);
                    let bytes = (r as i32).to_le_bytes();
                    data[off..off + 4].copy_from_slice(&bytes);
                    value::encode_f64(o as f64)
                }
                (4, 1) => {
                    let v = (value::decode_f64(value_raw) as u32) as i64;
                    let o = u32::from_le_bytes([
                        data[off],
                        data[off + 1],
                        data[off + 2],
                        data[off + 3],
                    ]) as i64;
                    let r = op(o, v);
                    let bytes = (r as u32).to_le_bytes();
                    data[off..off + 4].copy_from_slice(&bytes);
                    value::encode_f64(o as f64)
                }
                (8, 4) => {
                    let v_handle = value::decode_bigint_handle(value_raw) as usize;
                    let bigint_table = caller.data().bigint_table.lock().ok()?;
                    let v_bi = bigint_table.get(v_handle)?.clone();
                    let v_bytes = bigint_low_64_bytes(&v_bi);
                    let v64 = i64::from_le_bytes(v_bytes);
                    let o64 = i64::from_le_bytes([
                        data[off],
                        data[off + 1],
                        data[off + 2],
                        data[off + 3],
                        data[off + 4],
                        data[off + 5],
                        data[off + 6],
                        data[off + 7],
                    ]);
                    let r64 = op(o64, v64);
                    drop(bigint_table);
                    let r_bytes = r64.to_le_bytes();
                    data[off..off + 8].copy_from_slice(&r_bytes);
                    let mut bigint_table = caller.data().bigint_table.lock().ok()?;
                    let handle = bigint_table.len() as u32;
                    bigint_table.push(num_bigint::BigInt::from(o64));
                    value::encode_bigint_handle(handle)
                }
                (8, 5) => {
                    let v_handle = value::decode_bigint_handle(value_raw) as usize;
                    let bigint_table = caller.data().bigint_table.lock().ok()?;
                    let v_bi = bigint_table.get(v_handle)?.clone();
                    let v_bytes = bigint_low_64_bytes(&v_bi);
                    let v64 = u64::from_le_bytes(v_bytes) as i64;
                    let o_bytes: [u8; 8] = [
                        data[off],
                        data[off + 1],
                        data[off + 2],
                        data[off + 3],
                        data[off + 4],
                        data[off + 5],
                        data[off + 6],
                        data[off + 7],
                    ];
                    let o64 = u64::from_le_bytes(o_bytes) as i64;
                    let r64 = op(o64, v64);
                    let r_bytes = (r64 as u64).to_le_bytes();
                    data[off..off + 8].copy_from_slice(&r_bytes);
                    let mut bigint_table = caller.data().bigint_table.lock().ok()?;
                    let handle = bigint_table.len() as u32;
                    bigint_table.push(num_bigint::BigInt::from(o64 as u64));
                    value::encode_bigint_handle(handle)
                }
                _ => return None,
            };
            Some(old)
        }
    }

    /// 带锁的 SAB 读取，返回 decoder 配对。
    fn sab_locked_read(
        caller: &Caller<'_, RuntimeState>,
        sab_handle: usize,
        off: usize,
        elem_size: u8,
        element_kind: u8,
    ) -> Option<i64> {
        let shared = caller.data().shared_state.as_ref()?.clone();
        let table = shared.sab_table.lock().ok()?;
        let entry = table.get(sab_handle)?;
        let data = entry.data.read().ok()?;
        if off + elem_size as usize > data.len() {
            return None;
        }
        match (elem_size, element_kind) {
            (1, 0) => Some(value::encode_f64(data[off] as i8 as f64)),
            (1, 1) => Some(value::encode_f64(data[off] as f64)),
            (2, 0) => Some(value::encode_f64(
                i16::from_le_bytes([data[off], data[off + 1]]) as f64,
            )),
            (2, 1) => Some(value::encode_f64(
                u16::from_le_bytes([data[off], data[off + 1]]) as f64,
            )),
            (4, 0) => Some(value::encode_f64(i32::from_le_bytes([
                data[off],
                data[off + 1],
                data[off + 2],
                data[off + 3],
            ]) as f64)),
            (4, 1) => Some(value::encode_f64(u32::from_le_bytes([
                data[off],
                data[off + 1],
                data[off + 2],
                data[off + 3],
            ]) as f64)),
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
                Some(value::encode_bigint_handle(handle))
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
                Some(value::encode_bigint_handle(handle))
            }
            _ => None,
        }
    }

    // ── Atomics.load(typedArray, index) → value ──────────────────────
    let atomics_load_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index_val: i64, _: i64| -> i64 {
            let entry = match typedarray_entry_from_value(&mut caller, this_val) {
                Some(e) if e.element_kind != 2 && e.element_kind != 3 => e,
                _ => {
                    set_typedarray_runtime_error(
                        &mut caller,
                        "TypeError: Typed array is not an integer type for Atomics",
                    );
                    return value::encode_undefined();
                }
            };
            let index = match typedarray_to_index(
                &mut caller,
                index_val,
                "RangeError: Invalid typed array index",
            ) {
                Some(i) => i,
                None => return value::encode_undefined(),
            };
            if index >= entry.length {
                set_typedarray_runtime_error(&mut caller, "RangeError: Invalid typed array index");
                return value::encode_undefined();
            }
            typedarray_element_read_entry(&mut caller, &entry, index)
                .unwrap_or_else(value::encode_undefined)
        },
    );
    linker.define(&mut store, "env", "atomics_load", atomics_load_fn)?;

    // ── Atomics.store(typedArray, index, value) → value ──────────────
    let atomics_store_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         index_val: i64,
         value_val: i64|
         -> i64 {
            let entry = match typedarray_entry_from_value(&mut caller, this_val) {
                Some(e) if e.element_kind != 2 && e.element_kind != 3 => e,
                _ => {
                    set_typedarray_runtime_error(
                        &mut caller,
                        "TypeError: Typed array is not an integer type for Atomics",
                    );
                    return value::encode_undefined();
                }
            };
            let index = match typedarray_to_index(
                &mut caller,
                index_val,
                "RangeError: Invalid typed array index",
            ) {
                Some(i) => i,
                None => return value::encode_undefined(),
            };
            if index >= entry.length {
                set_typedarray_runtime_error(&mut caller, "RangeError: Invalid typed array index");
                return value::encode_undefined();
            }
            if !typedarray_element_write(&mut caller, this_val, index, value_val) {
                return value::encode_undefined();
            }
            value_val
        },
    );
    linker.define(&mut store, "env", "atomics_store", atomics_store_fn)?;

    macro_rules! atomics_rmw_fn {
        ($name:ident, $op:expr) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>,
                 this_val: i64,
                 index_val: i64,
                 value_val: i64|
                 -> i64 {
                    let (byte_offset, elem_size, element_kind, buf_handle, is_shared) =
                        match prepare_atomics_access(&mut caller, this_val, index_val) {
                            Some(v) => v,
                            None => return value::encode_undefined(),
                        };
                    let off = byte_offset;
                    match atomic_rmw(
                        &mut caller,
                        buf_handle,
                        off,
                        elem_size,
                        element_kind,
                        value_val,
                        is_shared,
                        $op,
                    ) {
                        Some(v) => v,
                        None => value::encode_undefined(),
                    }
                },
            );
        };
    }

    atomics_rmw_fn!(atomics_add_fn, |o, v| o.wrapping_add(v));
    linker.define(&mut store, "env", "atomics_add", atomics_add_fn)?;

    atomics_rmw_fn!(atomics_sub_fn, |o, v| o.wrapping_sub(v));
    linker.define(&mut store, "env", "atomics_sub", atomics_sub_fn)?;

    atomics_rmw_fn!(atomics_and_fn, |o, v| o & v);
    linker.define(&mut store, "env", "atomics_and", atomics_and_fn)?;

    atomics_rmw_fn!(atomics_or_fn, |o, v| o | v);
    linker.define(&mut store, "env", "atomics_or", atomics_or_fn)?;

    atomics_rmw_fn!(atomics_xor_fn, |o, v| o ^ v);
    linker.define(&mut store, "env", "atomics_xor", atomics_xor_fn)?;

    // ── Atomics.exchange ──
    let atomics_exchange_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         index_val: i64,
         value_val: i64|
         -> i64 {
            let (byte_offset, elem_size, element_kind, buf_handle, is_shared) =
                match prepare_atomics_access(&mut caller, this_val, index_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let off = byte_offset;
            match atomic_rmw(
                &mut caller,
                buf_handle,
                off,
                elem_size,
                element_kind,
                value_val,
                is_shared,
                |_o, v| v,
            ) {
                Some(v) => v,
                None => value::encode_undefined(),
            }
        },
    );
    linker.define(&mut store, "env", "atomics_exchange", atomics_exchange_fn)?;

    // ── Atomics.compareExchange(typedArray, index, expected, replacement) → old value ─
    let atomics_compare_exchange_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         index_val: i64,
         expected_val: i64,
         replacement_val: i64|
         -> i64 {
            let (byte_offset, elem_size, element_kind, buf_handle, is_shared) =
                match prepare_atomics_access(&mut caller, this_val, index_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let off = byte_offset;
            if is_shared {
                let shared = match caller.data().shared_state.as_ref().cloned() {
                    Some(s) => s,
                    None => return value::encode_undefined(),
                };
                let table = match shared.sab_table.lock() {
                    Ok(t) => t,
                    Err(_) => return value::encode_undefined(),
                };
                let entry = match table.get(buf_handle) {
                    Some(e) => e,
                    None => return value::encode_undefined(),
                };
                let mut data = match entry.data.write() {
                    Ok(d) => d,
                    Err(_) => return value::encode_undefined(),
                };
                if off + elem_size as usize > data.len() {
                    return value::encode_undefined();
                }
                match (elem_size, element_kind) {
                    (1, 0) => {
                        let expected = value::decode_f64(expected_val) as i8;
                        let replacement = value::decode_f64(replacement_val) as i8;
                        let o = data[off] as i8;
                        if o == expected {
                            data[off] = replacement as u8;
                        }
                        value::encode_f64(o as f64)
                    }
                    (1, 1) => {
                        let expected = value::decode_f64(expected_val) as u8;
                        let replacement = value::decode_f64(replacement_val) as u8;
                        let o = data[off];
                        if o == expected {
                            data[off] = replacement;
                        }
                        value::encode_f64(o as f64)
                    }
                    (2, 0) => {
                        let expected = value::decode_f64(expected_val) as i16;
                        let replacement = value::decode_f64(replacement_val) as i16;
                        let o = i16::from_le_bytes([data[off], data[off + 1]]);
                        if o == expected {
                            data[off..off + 2].copy_from_slice(&replacement.to_le_bytes());
                        }
                        value::encode_f64(o as f64)
                    }
                    (2, 1) => {
                        let expected = value::decode_f64(expected_val) as u16;
                        let replacement = value::decode_f64(replacement_val) as u16;
                        let o = u16::from_le_bytes([data[off], data[off + 1]]);
                        if o == expected {
                            data[off..off + 2].copy_from_slice(&replacement.to_le_bytes());
                        }
                        value::encode_f64(o as f64)
                    }
                    (4, 0) => {
                        let expected = value::decode_f64(expected_val) as i32;
                        let replacement = value::decode_f64(replacement_val) as i32;
                        let o = i32::from_le_bytes([
                            data[off],
                            data[off + 1],
                            data[off + 2],
                            data[off + 3],
                        ]);
                        if o == expected {
                            data[off..off + 4].copy_from_slice(&replacement.to_le_bytes());
                        }
                        value::encode_f64(o as f64)
                    }
                    (4, 1) => {
                        let expected = value::decode_f64(expected_val) as u32;
                        let replacement = value::decode_f64(replacement_val) as u32;
                        let o = u32::from_le_bytes([
                            data[off],
                            data[off + 1],
                            data[off + 2],
                            data[off + 3],
                        ]);
                        if o == expected {
                            data[off..off + 4].copy_from_slice(&replacement.to_le_bytes());
                        }
                        value::encode_f64(o as f64)
                    }
                    (8, 4) => {
                        // BigInt value compare (not handle); alloc return handle after data scope
                        let expected_handle = value::decode_bigint_handle(expected_val) as usize;
                        let replacement_handle =
                            value::decode_bigint_handle(replacement_val) as usize;
                        let bigint_table = match caller.data().bigint_table.lock() {
                            Ok(t) => t,
                            Err(_) => return value::encode_undefined(),
                        };
                        let expected_bi = match bigint_table.get(expected_handle) {
                            Some(b) => b.clone(),
                            None => return value::encode_undefined(),
                        };
                        let replacement_bi = match bigint_table.get(replacement_handle) {
                            Some(b) => b.clone(),
                            None => return value::encode_undefined(),
                        };
                        let expected_bytes = bigint_low_64_bytes(&expected_bi);
                        let replacement_bytes = bigint_low_64_bytes(&replacement_bi);
                        let expected64 = i64::from_le_bytes(expected_bytes);
                        drop(bigint_table);
                        let o64 = i64::from_le_bytes([
                            data[off],
                            data[off + 1],
                            data[off + 2],
                            data[off + 3],
                            data[off + 4],
                            data[off + 5],
                            data[off + 6],
                            data[off + 7],
                        ]);
                        if o64 == expected64 {
                            data[off..off + 8].copy_from_slice(&replacement_bytes);
                        }
                        // drop data guard by ending scope before re-locking bigint for return handle
                        drop(data);
                        let mut bigint_table = match caller.data().bigint_table.lock() {
                            Ok(t) => t,
                            Err(_) => return value::encode_undefined(),
                        };
                        let handle = bigint_table.len() as u32;
                        bigint_table.push(num_bigint::BigInt::from(o64));
                        value::encode_bigint_handle(handle)
                    }
                    (8, 5) => {
                        let expected_handle = value::decode_bigint_handle(expected_val) as usize;
                        let replacement_handle =
                            value::decode_bigint_handle(replacement_val) as usize;
                        let bigint_table = match caller.data().bigint_table.lock() {
                            Ok(t) => t,
                            Err(_) => return value::encode_undefined(),
                        };
                        let expected_bi = match bigint_table.get(expected_handle) {
                            Some(b) => b.clone(),
                            None => return value::encode_undefined(),
                        };
                        let replacement_bi = match bigint_table.get(replacement_handle) {
                            Some(b) => b.clone(),
                            None => return value::encode_undefined(),
                        };
                        let expected_bytes = bigint_low_64_bytes(&expected_bi);
                        let replacement_bytes = bigint_low_64_bytes(&replacement_bi);
                        let expected64 = u64::from_le_bytes(expected_bytes);
                        drop(bigint_table);
                        let o64 = u64::from_le_bytes([
                            data[off],
                            data[off + 1],
                            data[off + 2],
                            data[off + 3],
                            data[off + 4],
                            data[off + 5],
                            data[off + 6],
                            data[off + 7],
                        ]);
                        if o64 == expected64 {
                            data[off..off + 8].copy_from_slice(&replacement_bytes);
                        }
                        drop(data);
                        let mut bigint_table = match caller.data().bigint_table.lock() {
                            Ok(t) => t,
                            Err(_) => return value::encode_undefined(),
                        };
                        let handle = bigint_table.len() as u32;
                        bigint_table.push(num_bigint::BigInt::from(o64));
                        value::encode_bigint_handle(handle)
                    }
                    _ => value::encode_undefined(),
                }
            } else {
                // AB path (non-shared)
                let mut ab_table = match caller.data().arraybuffer_table.lock() {
                    Ok(t) => t,
                    Err(_) => return value::encode_undefined(),
                };
                let ab_entry = match ab_table.get_mut(buf_handle) {
                    Some(e) => e,
                    None => return value::encode_undefined(),
                };
                let data = &mut ab_entry.data;
                if off + elem_size as usize > data.len() {
                    return value::encode_undefined();
                }
                match (elem_size, element_kind) {
                    (1, 0) => {
                        let expected = value::decode_f64(expected_val) as i8;
                        let replacement = value::decode_f64(replacement_val) as i8;
                        let o = data[off] as i8;
                        if o == expected {
                            data[off] = replacement as u8;
                        }
                        value::encode_f64(o as f64)
                    }
                    (1, 1) => {
                        let expected = value::decode_f64(expected_val) as u8;
                        let replacement = value::decode_f64(replacement_val) as u8;
                        let o = data[off];
                        if o == expected {
                            data[off] = replacement;
                        }
                        value::encode_f64(o as f64)
                    }
                    (2, 0) => {
                        let expected = value::decode_f64(expected_val) as i16;
                        let replacement = value::decode_f64(replacement_val) as i16;
                        let o = i16::from_le_bytes([data[off], data[off + 1]]);
                        if o == expected {
                            data[off..off + 2].copy_from_slice(&replacement.to_le_bytes());
                        }
                        value::encode_f64(o as f64)
                    }
                    (2, 1) => {
                        let expected = value::decode_f64(expected_val) as u16;
                        let replacement = value::decode_f64(replacement_val) as u16;
                        let o = u16::from_le_bytes([data[off], data[off + 1]]);
                        if o == expected {
                            data[off..off + 2].copy_from_slice(&replacement.to_le_bytes());
                        }
                        value::encode_f64(o as f64)
                    }
                    (4, 0) => {
                        let expected = value::decode_f64(expected_val) as i32;
                        let replacement = value::decode_f64(replacement_val) as i32;
                        let o = i32::from_le_bytes([
                            data[off],
                            data[off + 1],
                            data[off + 2],
                            data[off + 3],
                        ]);
                        if o == expected {
                            data[off..off + 4].copy_from_slice(&replacement.to_le_bytes());
                        }
                        value::encode_f64(o as f64)
                    }
                    (4, 1) => {
                        let expected = value::decode_f64(expected_val) as u32;
                        let replacement = value::decode_f64(replacement_val) as u32;
                        let o = u32::from_le_bytes([
                            data[off],
                            data[off + 1],
                            data[off + 2],
                            data[off + 3],
                        ]);
                        if o == expected {
                            data[off..off + 4].copy_from_slice(&replacement.to_le_bytes());
                        }
                        value::encode_f64(o as f64)
                    }
                    (8, 4) => {
                        let expected_handle = value::decode_bigint_handle(expected_val) as usize;
                        let replacement_handle =
                            value::decode_bigint_handle(replacement_val) as usize;
                        let bigint_table = match caller.data().bigint_table.lock() {
                            Ok(t) => t,
                            Err(_) => return value::encode_undefined(),
                        };
                        let expected_bi = match bigint_table.get(expected_handle) {
                            Some(b) => b.clone(),
                            None => return value::encode_undefined(),
                        };
                        let replacement_bi = match bigint_table.get(replacement_handle) {
                            Some(b) => b.clone(),
                            None => return value::encode_undefined(),
                        };
                        let expected_bytes = bigint_low_64_bytes(&expected_bi);
                        let replacement_bytes = bigint_low_64_bytes(&replacement_bi);
                        let expected64 = i64::from_le_bytes(expected_bytes);
                        drop(bigint_table);
                        let o64 = i64::from_le_bytes([
                            data[off],
                            data[off + 1],
                            data[off + 2],
                            data[off + 3],
                            data[off + 4],
                            data[off + 5],
                            data[off + 6],
                            data[off + 7],
                        ]);
                        if o64 == expected64 {
                            data[off..off + 8].copy_from_slice(&replacement_bytes);
                        }
                        let _ = data;
                        let mut bigint_table = match caller.data().bigint_table.lock() {
                            Ok(t) => t,
                            Err(_) => return value::encode_undefined(),
                        };
                        let handle = bigint_table.len() as u32;
                        bigint_table.push(num_bigint::BigInt::from(o64));
                        value::encode_bigint_handle(handle)
                    }
                    (8, 5) => {
                        let expected_handle = value::decode_bigint_handle(expected_val) as usize;
                        let replacement_handle =
                            value::decode_bigint_handle(replacement_val) as usize;
                        let bigint_table = match caller.data().bigint_table.lock() {
                            Ok(t) => t,
                            Err(_) => return value::encode_undefined(),
                        };
                        let expected_bi = match bigint_table.get(expected_handle) {
                            Some(b) => b.clone(),
                            None => return value::encode_undefined(),
                        };
                        let replacement_bi = match bigint_table.get(replacement_handle) {
                            Some(b) => b.clone(),
                            None => return value::encode_undefined(),
                        };
                        let expected_bytes = bigint_low_64_bytes(&expected_bi);
                        let replacement_bytes = bigint_low_64_bytes(&replacement_bi);
                        let expected64 = u64::from_le_bytes(expected_bytes);
                        drop(bigint_table);
                        let o64 = u64::from_le_bytes([
                            data[off],
                            data[off + 1],
                            data[off + 2],
                            data[off + 3],
                            data[off + 4],
                            data[off + 5],
                            data[off + 6],
                            data[off + 7],
                        ]);
                        if o64 == expected64 {
                            data[off..off + 8].copy_from_slice(&replacement_bytes);
                        }
                        let _ = data;
                        let mut bigint_table = match caller.data().bigint_table.lock() {
                            Ok(t) => t,
                            Err(_) => return value::encode_undefined(),
                        };
                        let handle = bigint_table.len() as u32;
                        bigint_table.push(num_bigint::BigInt::from(o64));
                        value::encode_bigint_handle(handle)
                    }
                    _ => value::encode_undefined(),
                }
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "atomics_compare_exchange",
        atomics_compare_exchange_fn,
    )?;

    // ── Atomics.isLockFree(size) → bool ──────────────────────────────
    let atomics_is_lock_free_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, size_val: i64| -> i64 {
            let size = value::decode_f64(size_val) as u8;
            #[allow(clippy::match_like_matches_macro)]
            let result = match size {
                1 | 2 | 4 => true,
                8 => cfg!(target_has_atomic = "64"),
                _ => false,
            };
            if result {
                value::encode_bool(true)
            } else {
                value::encode_bool(false)
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "atomics_is_lock_free",
        atomics_is_lock_free_fn,
    )?;

    let atomics_pause_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>| -> i64 {
        crate::shared_buffer::atomics_pause(&mut caller)
    });
    linker.define(&mut store, "env", "atomics_pause", atomics_pause_fn)?;

    // ── Atomics.wait(typedArray, index, expected, timeout) → string ──
    linker.func_wrap_async(
        "env",
        "atomics_wait",
        |mut caller: Caller<'_, RuntimeState>,
         (this_val, index_val, expected_val, timeout_val): (i64, i64, i64, i64)|
         {
            let (byte_offset, elem_size, element_kind, buf_handle) =
                match prepare_waitable_access(&mut caller, this_val, index_val) {
                    Some(v) => v,
                    None => return Box::new(async move { value::encode_undefined() }),
                };
            let off = byte_offset;
            let current = match sab_locked_read(&caller, buf_handle, off, elem_size, element_kind) {
                Some(v) => v,
                None => return Box::new(async move { value::encode_undefined() }),
            };
            let equal = if element_kind >= 4 {
                // BigInt: compare by value not handle
                if value::is_bigint(current) && value::is_bigint(expected_val) {
                    let h1 = value::decode_bigint_handle(current) as usize;
                    let h2 = value::decode_bigint_handle(expected_val) as usize;
                    let bt = caller.data().bigint_table.lock().ok();
                    if let Some(table) = bt {
                        let b1 = table.get(h1);
                        let b2 = table.get(h2);
                        b1 == b2 || (b1.is_some() && b2.is_some() && b1.unwrap() == b2.unwrap())
                    } else { false }
                } else { false }
            } else {
                current == expected_val
            };
            if !equal {
                let s = store_runtime_string(&caller, "not-equal".to_string());
                return Box::new(async move { s });
            }
            let tmo = if value::is_undefined(timeout_val) {
                f64::INFINITY
            } else if value::is_f64(timeout_val) {
                value::decode_f64(timeout_val)
            } else {
                0.0
            };
            if tmo <= 0.0 {
                let s = store_runtime_string(&caller, "timed-out".to_string());
                return Box::new(async move { s });
            }
            // enqueue for blocking wait (promise=None for sync wait path)
            let shared = match caller.data().shared_state.clone() {
                Some(s) => s,
                None => {
                    let s = store_runtime_string(&caller, "timed-out".to_string());
                    return Box::new(async move { s });
                }
            };
            let dl = if tmo.is_infinite() {
                None
            } else {
                Some(::tokio::time::Instant::now() + ::std::time::Duration::from_millis(tmo.max(0.0) as u64))
            };
            let waiter = crate::shared_buffer::enter_waiter(&shared, buf_handle as u32, off as u32, dl, None);
            let shared2 = shared.clone();
            let buf_h = buf_handle as u32;
            let off_u = off as u32;
            Box::new(async move {
                let status = if let Some(d) = dl {
                    tokio::select! {
                        _ = waiter.signal.notified() => "ok",
                        _ = ::tokio::time::sleep_until(d) => {
                            crate::shared_buffer::remove_waiter(&shared2, buf_h, off_u, &waiter.notified);
                            if waiter.notified.load(::std::sync::atomic::Ordering::SeqCst) {
                                "ok"
                            } else {
                                "timed-out"
                            }
                        }
                    }
                } else {
                    waiter.signal.notified().await;
                    "ok"
                };
                store_runtime_string(&caller, status.to_string())
            })
        },
    )?;
    let atomics_notify_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         index_val: i64,
         count_val: i64|
         -> i64 {
            let (byte_offset, _elem_size, _element_kind, sab_handle) =
                match prepare_waitable_access(&mut caller, this_val, index_val) {
                    Some(v) => v,
                    None => return value::encode_f64(0.0),
                };
            let count = if value::is_undefined(count_val) {
                u32::MAX
            } else if value::is_bigint(count_val) {
                0
            } else {
                let n = value::decode_f64(count_val);
                if !n.is_finite() {
                    if n > 0.0 { u32::MAX } else { 0 }
                } else if n <= 0.0 {
                    0
                } else {
                    n.trunc() as u32
                }
            };
            let shared = match caller.data().shared_state.as_ref().cloned() {
                Some(s) => s,
                None => return value::encode_f64(0.0),
            };
            let (woken, promises) = crate::shared_buffer::notify_waiters_with_promises(
                &shared,
                sab_handle as u32,
                byte_offset as u32,
                count,
            );
            for pr in promises {
                let ok_str = store_runtime_string(&caller, "ok".to_string());
                settle_promise(caller.data_mut(), pr, PromiseSettlement::Fulfill(ok_str));
            }
            value::encode_f64(woken as f64)
        },
    );
    linker.define(&mut store, "env", "atomics_notify", atomics_notify_fn)?;

    // ── Atomics.waitAsync(typedArray, index, value, timeout) → object ─
    let atomics_wait_async_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         index_val: i64,
         value_val: i64,
         _timeout_val: i64|
         -> i64 {
            let (byte_offset, elem_size, element_kind, buf_handle) =
                match prepare_waitable_access(&mut caller, this_val, index_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let off = byte_offset;
            let current = match sab_locked_read(&caller, buf_handle, off, elem_size, element_kind) {
                Some(v) => v,
                None => return value::encode_undefined(),
            };
            let equal = if element_kind >= 4 {
                if value::is_bigint(current) && value::is_bigint(value_val) {
                    let h1 = value::decode_bigint_handle(current) as usize;
                    let h2 = value::decode_bigint_handle(value_val) as usize;
                    let bt = caller.data().bigint_table.lock().ok();
                    if let Some(table) = bt {
                        let b1 = table.get(h1);
                        let b2 = table.get(h2);
                        b1 == b2 || (b1.is_some() && b2.is_some() && b1.unwrap() == b2.unwrap())
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                current == value_val
            };
            let tmo = if value::is_undefined(_timeout_val) {
                f64::INFINITY
            } else if value::is_f64(_timeout_val) {
                value::decode_f64(_timeout_val)
            } else {
                0.0
            };
            if !equal || tmo <= 0.0 {
                let result = {
                    let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                    alloc_host_object(&mut caller, &_wjsm_env, 4)
                };
                let _ = define_host_data_property_from_caller(
                    &mut caller,
                    result,
                    "async",
                    value::encode_bool(false),
                );
                let status = if !equal { "not-equal" } else { "timed-out" };
                let status_str = store_runtime_string(&caller, status.to_string());
                let _ =
                    define_host_data_property_from_caller(&mut caller, result, "value", status_str);
                return result;
            }
            // tmo > 0 and equal: full async path — return real Promise, enqueue with promise handle, schedule timeout settlement via tx
            let dl = if tmo.is_infinite() {
                None
            } else {
                Some(
                    ::tokio::time::Instant::now()
                        + ::std::time::Duration::from_millis(tmo.max(0.0) as u64),
                )
            };
            let promise = alloc_promise_from_caller(&mut caller, PromiseEntry::pending());
            let shared = match caller.data().shared_state.clone() {
                Some(s) => s,
                None => {
                    let result = {
                        let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                        alloc_host_object(&mut caller, &_wjsm_env, 4)
                    };
                    let _ = define_host_data_property_from_caller(
                        &mut caller,
                        result,
                        "async",
                        value::encode_bool(true),
                    );
                    let _ = define_host_data_property_from_caller(
                        &mut caller,
                        result,
                        "value",
                        promise,
                    );
                    return result;
                }
            };
            let notified = crate::shared_buffer::enter_waiter(
                &shared,
                buf_handle as u32,
                off as u32,
                dl,
                Some(promise),
            );
            if let Some(d) = dl
                && let Some(tx) = caller.data().host_completion_tx.clone()
            {
                let sh = shared.clone();
                let waiter = notified.clone();
                let nh = waiter.notified.clone();
                let pclone = promise;
                let bh = buf_handle as u32;
                let ou = off as u32;
                let scope = crate::scheduler::capture_completion_scope_from_caller(&caller);
                tokio::spawn(async move {
                    ::tokio::time::sleep_until(d).await;
                    crate::shared_buffer::remove_waiter(&sh, bh, ou, &nh);
                    let _ = tx.send(crate::scheduler::AsyncHostCompletion::Materialize {
                        promise: pclone,
                        materialize: Box::new(move |store, _env| {
                            let timed = crate::runtime_render::store_runtime_string_in_state(
                                store.data(),
                                "timed-out".to_string(),
                            );
                            PromiseSettlement::Fulfill(timed)
                        }),
                        scope,
                    });
                });
            }
            let result = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 4)
            };
            let _ = define_host_data_property_from_caller(
                &mut caller,
                result,
                "async",
                value::encode_bool(true),
            );
            let _ = define_host_data_property_from_caller(&mut caller, result, "value", promise);
            result
        },
    );
    linker.define(
        &mut store,
        "env",
        "atomics_wait_async",
        atomics_wait_async_fn,
    )?;

    // ── SharedArrayBuffer (delegates to shared_buffer owner) ──
    let sab_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, length: i64, options: i64, target_obj: i64| -> i64 {
            crate::shared_buffer::construct_shared_array_buffer(
                &mut caller,
                length,
                options,
                target_obj,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "sharedarraybuffer_constructor",
        sab_constructor_fn,
    )?;
    let sab_byte_length_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            crate::shared_buffer::shared_array_buffer_byte_length(&mut caller, this_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "sharedarraybuffer_proto_byte_length",
        sab_byte_length_fn,
    )?;
    let sab_slice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, begin: i64, end: i64| -> i64 {
            crate::shared_buffer::shared_array_buffer_slice(&mut caller, this_val, begin, end)
        },
    );
    linker.define(
        &mut store,
        "env",
        "sharedarraybuffer_proto_slice",
        sab_slice_fn,
    )?;
    let sab_grow_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, new_length: i64| -> i64 {
            crate::shared_buffer::shared_array_buffer_grow(&mut caller, this_val, new_length)
        },
    );
    linker.define(
        &mut store,
        "env",
        "sharedarraybuffer_proto_grow",
        sab_grow_fn,
    )?;
    let sab_growable_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            crate::shared_buffer::shared_array_buffer_growable(&mut caller, this_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "sharedarraybuffer_proto_growable",
        sab_growable_fn,
    )?;
    let sab_max_byte_length_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            crate::shared_buffer::shared_array_buffer_max_byte_length(&mut caller, this_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "sharedarraybuffer_proto_max_byte_length",
        sab_max_byte_length_fn,
    )?;
    let sab_species_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            crate::shared_buffer::shared_array_buffer_species(&mut caller, this_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "sharedarraybuffer_proto_species",
        sab_species_fn,
    )?;
    Ok(())
}
