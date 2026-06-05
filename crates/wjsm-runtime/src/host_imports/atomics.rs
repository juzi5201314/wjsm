use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::*;

pub(crate) fn define_atomics(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    /// 验证 |this_val| 是整数 TypedArray（含 BigInt typed arrays）。
    /// element_kind: 0=i8, 1=u8, 2=u8clamped, 3=float, 4=BigInt64, 5=BigUint64。
    /// 拒绝 element_kind 3（float）和 2（clamped 不是规范 Atomics 整数类型）。
    fn validate_ta_for_atomics(
        caller: &mut Caller<'_, RuntimeState>,
        this_val: i64,
    ) -> Option<(usize, u8, u8, usize)> {
        if !value::is_object(this_val) {
            return None;
        }
        let obj_ptr =
            resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize)?;
        let h = read_object_property_by_name(caller, obj_ptr, "__typedarray_handle__")?;
        let handle = value::decode_f64(h) as usize;
        let table = caller.data().typedarray_table.lock().ok()?;
        let entry = table.get(handle)?;
        if entry.element_kind == 2 || entry.element_kind == 3 {
            return None;
        }
        let sab_handle = entry.buffer_handle as usize;
        let byte_offset = entry.byte_offset as usize;
        Some((byte_offset, entry.element_size, entry.element_kind, sab_handle))
    }

    /// 验证 TypedArray 对 wait/waitAsync/notify 可用（仅 Int32 或 BigInt64）。
    fn validate_waitable_ta(
        caller: &mut Caller<'_, RuntimeState>,
        this_val: i64,
    ) -> Option<(usize, u8, u8, usize)> {
        let (byte_offset, elem_size, element_kind, sab_handle) =
            validate_ta_for_atomics(caller, this_val)?;
        let waitable = (elem_size == 4 && element_kind == 0)
            || (elem_size == 8 && element_kind == 4);
        if !waitable {
            return None;
        }
        Some((byte_offset, elem_size, element_kind, sab_handle))
    }

    /// 对 SAB 数据加锁，执行字节级原子操作，返回旧值（i64）。
    /// element_kind 0/1: Number 类型，旧值返回为 f64；4/5: BigInt 类型，旧值返回为 bigint handle。
    fn atomic_rmw(
        caller: &mut Caller<'_, RuntimeState>,
        sab_handle: usize,
        off: usize,
        elem_size: u8,
        element_kind: u8,
        value_raw: i64,
        op: fn(i64, i64) -> i64,
    ) -> Option<i64> {
        let shared = caller.data().shared_state.as_ref()?.clone();
        let table = shared.sab_table.lock().ok()?;
        let entry = table.get(sab_handle)?;
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
                    data[off], data[off + 1], data[off + 2], data[off + 3],
                ]) as i64;
                let r = op(o, v);
                let bytes = (r as i32).to_le_bytes();
                data[off..off + 4].copy_from_slice(&bytes);
                value::encode_f64(o as f64)
            }
            (4, 1) => {
                let v = (value::decode_f64(value_raw) as u32) as i64;
                let o = u32::from_le_bytes([
                    data[off], data[off + 1], data[off + 2], data[off + 3],
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
                    data[off], data[off + 1], data[off + 2], data[off + 3],
                    data[off + 4], data[off + 5], data[off + 6], data[off + 7],
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
                    data[off], data[off + 1], data[off + 2], data[off + 3],
                    data[off + 4], data[off + 5], data[off + 6], data[off + 7],
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
            (2, 0) => {
                Some(value::encode_f64(
                    i16::from_le_bytes([data[off], data[off + 1]]) as f64,
                ))
            }
            (2, 1) => {
                Some(value::encode_f64(
                    u16::from_le_bytes([data[off], data[off + 1]]) as f64,
                ))
            }
            (4, 0) => Some(value::encode_f64(
                i32::from_le_bytes([
                    data[off], data[off + 1], data[off + 2], data[off + 3],
                ]) as f64,
            )),
            (4, 1) => Some(value::encode_f64(
                u32::from_le_bytes([
                    data[off], data[off + 1], data[off + 2], data[off + 3],
                ]) as f64,
            )),
            (8, 4) => {
                let v = i64::from_le_bytes([
                    data[off], data[off + 1], data[off + 2], data[off + 3],
                    data[off + 4], data[off + 5], data[off + 6], data[off + 7],
                ]);
                let mut table = caller.data().bigint_table.lock().ok()?;
                let handle = table.len() as u32;
                table.push(num_bigint::BigInt::from(v));
                Some(value::encode_bigint_handle(handle))
            }
            (8, 5) => {
                let v = u64::from_le_bytes([
                    data[off], data[off + 1], data[off + 2], data[off + 3],
                    data[off + 4], data[off + 5], data[off + 6], data[off + 7],
                ]);
                let mut table = caller.data().bigint_table.lock().ok()?;
                let handle = table.len() as u32;
                table.push(num_bigint::BigInt::from(v));
                Some(value::encode_bigint_handle(handle))
            }
            _ => None,
        }
    }

    /// 带锁的 SAB 写入。val 均为 f64 encode（Number 数组）或 bigint handle（BigInt 数组）。
    fn sab_locked_write(
        caller: &Caller<'_, RuntimeState>,
        sab_handle: usize,
        off: usize,
        elem_size: u8,
        element_kind: u8,
        val: i64,
    ) -> Option<()> {
        let shared = caller.data().shared_state.as_ref()?.clone();
        let table = shared.sab_table.lock().ok()?;
        let entry = table.get(sab_handle)?;
        let mut data = entry.data.write().ok()?;
        if off + elem_size as usize > data.len() {
            return None;
        }
        match (elem_size, element_kind) {
            (1, 0) => {
                data[off] = (value::decode_f64(val) as i8) as u8;
                Some(())
            }
            (1, 1) => {
                data[off] = value::decode_f64(val) as u8;
                Some(())
            }
            (2, 0) => {
                let bytes = (value::decode_f64(val) as i16).to_le_bytes();
                data[off..off + 2].copy_from_slice(&bytes);
                Some(())
            }
            (2, 1) => {
                let bytes = (value::decode_f64(val) as u16).to_le_bytes();
                data[off..off + 2].copy_from_slice(&bytes);
                Some(())
            }
            (4, 0) => {
                let bytes = (value::decode_f64(val) as i32).to_le_bytes();
                data[off..off + 4].copy_from_slice(&bytes);
                Some(())
            }
            (4, 1) => {
                let bytes = (value::decode_f64(val) as u32).to_le_bytes();
                data[off..off + 4].copy_from_slice(&bytes);
                Some(())
            }
            (8, 4) => {
                let handle = value::decode_bigint_handle(val) as usize;
                let bigint_table = caller.data().bigint_table.lock().ok()?;
                let bi = bigint_table.get(handle)?.clone();
                let bytes = bigint_low_64_bytes(&bi);
                data[off..off + 8].copy_from_slice(&bytes);
                Some(())
            }
            (8, 5) => {
                let handle = value::decode_bigint_handle(val) as usize;
                let bigint_table = caller.data().bigint_table.lock().ok()?;
                let bi = bigint_table.get(handle)?.clone();
                let bytes = bigint_low_64_bytes(&bi);
                data[off..off + 8].copy_from_slice(&bytes);
                Some(())
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
                _ => return value::encode_undefined(),
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
                _ => return value::encode_undefined(),
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
                    let (byte_offset, elem_size, element_kind, sab_handle) =
                        match validate_ta_for_atomics(&mut caller, this_val) {
                            Some(v) => v,
                            None => return value::encode_undefined(),
                        };
                    let index = value::decode_f64(index_val) as u32;
                    let off = byte_offset + (index as usize) * (elem_size as usize);
                    match atomic_rmw(
                        &mut caller,
                        sab_handle,
                        off,
                        elem_size,
                        element_kind,
                        value_val,
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
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            match atomic_rmw(&mut caller, sab_handle, off, elem_size, element_kind, value_val, |_o, v| v)
            {
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
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let shared = match caller.data().shared_state.as_ref().cloned() {
                Some(s) => s,
                None => return value::encode_undefined(),
            };
            let table = match shared.sab_table.lock() {
                Ok(t) => t,
                Err(_) => return value::encode_undefined(),
            };
            let entry = match table.get(sab_handle) {
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
                        data[off], data[off + 1], data[off + 2], data[off + 3],
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
                        data[off], data[off + 1], data[off + 2], data[off + 3],
                    ]);
                    if o == expected {
                        data[off..off + 4].copy_from_slice(&replacement.to_le_bytes());
                    }
                    value::encode_f64(o as f64)
                }
                (8, 4) => {
                    let expected_handle = value::decode_bigint_handle(expected_val) as usize;
                    let replacement_handle = value::decode_bigint_handle(replacement_val) as usize;
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
                        data[off], data[off + 1], data[off + 2], data[off + 3],
                        data[off + 4], data[off + 5], data[off + 6], data[off + 7],
                    ]);
                    if o64 == expected64 {
                        data[off..off + 8].copy_from_slice(&replacement_bytes);
                    }
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
                    let replacement_handle = value::decode_bigint_handle(replacement_val) as usize;
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
                        data[off], data[off + 1], data[off + 2], data[off + 3],
                        data[off + 4], data[off + 5], data[off + 6], data[off + 7],
                    ]);
                    if o64 == expected64 {
                        data[off..off + 8].copy_from_slice(&replacement_bytes);
                    }
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

    let atomics_pause_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>| -> i64 {
            crate::shared_buffer::atomics_pause(&mut caller)
        },
    );
    linker.define(&mut store, "env", "atomics_pause", atomics_pause_fn)?;

    // ── Atomics.wait(typedArray, index, expected, timeout) → string ──
    let atomics_wait_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         index_val: i64,
         expected_val: i64,
         _timeout_val: i64|
         -> i64 {
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_waitable_ta(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let current = match sab_locked_read(&caller, sab_handle, off, elem_size, element_kind) {
                Some(v) => v,
                None => return value::encode_undefined(),
            };
            if current != expected_val {
                return store_runtime_string(&mut caller, "not-equal".to_string());
            }
            store_runtime_string(&mut caller, "timed-out".to_string())
        },
    );
    linker.define(&mut store, "env", "atomics_wait", atomics_wait_fn)?;

    // ── Atomics.notify(typedArray, index, count) → i64 ───────────────
    let atomics_notify_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         _index_val: i64,
         _count_val: i64|
         -> i64 {
            let (_byte_offset, elem_size, element_kind, _sab_handle) =
                match validate_waitable_ta(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            drop((elem_size, element_kind));
            value::encode_f64(0.0)
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
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_waitable_ta(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let current = match sab_locked_read(&caller, sab_handle, off, elem_size, element_kind) {
                Some(v) => v,
                None => return value::encode_undefined(),
            };
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
            let status = if current != value_val {
                "not-equal"
            } else {
                "timed-out"
            };
            let status_str = store_runtime_string(&mut caller, status.to_string());
            let _ =
                define_host_data_property_from_caller(&mut caller, result, "value", status_str);
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
