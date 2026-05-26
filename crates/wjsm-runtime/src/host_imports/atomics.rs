{
    use std::sync::atomic::Ordering;

    /// Validate that |this_val| is a TypedArray backed by a SharedArrayBuffer.
    /// Returns (byte_offset, elem_size, element_kind, sab_handle) on success.
    fn validate_ta_for_atomics(
        caller: &mut Caller<'_, RuntimeState>,
        this_val: i64,
    ) -> Option<(usize, u8, u8, usize)> {
        if !value::is_object(this_val) {
            return None;
        }
        let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize)?;
        let h = read_object_property_by_name(caller, obj_ptr, "__typedarray_handle__")?;
        let handle = value::decode_f64(h) as usize;
        let table = caller.data().typedarray_table.lock().ok()?;
        let entry = table.get(handle)?;
        if !entry.is_shared {
            return None;
        }
        // Atomics only supports integer TypedArrays (element_kind 0, 1, 2).
        // element_kind 3 = float types (Float32, Float64, Float16) are not valid.
        if entry.element_kind >= 3 {
            return None;
        }
        let sab_handle = entry.buffer_handle as usize;
        let byte_offset = entry.byte_offset as usize;
        Some((byte_offset, entry.element_size, entry.element_kind, sab_handle))
    }

    /// Return a const raw pointer to the SAB data given a buffer handle.
    fn sab_data_ptr(caller: &Caller<'_, RuntimeState>, sab_handle: usize) -> Option<*const u8> {
        let shared = caller.data().shared_state.as_ref()?;
        let table = shared.sab_table.lock().ok()?;
        let entry = table.get(sab_handle)?;
        let ptr = entry.data.read().ok()?.as_ptr();
        Some(ptr)
    }

    /// Return a mutable raw pointer to the SAB data given a buffer handle.
    fn sab_data_mut_ptr(caller: &mut Caller<'_, RuntimeState>, sab_handle: usize) -> Option<*mut u8> {
        let shared = caller.data().shared_state.as_ref()?;
        let table = shared.sab_table.lock().ok()?;
        let entry = table.get(sab_handle)?;
        let ptr = entry.data.write().ok()?.as_mut_ptr();
        Some(ptr)
    }

    // ── Atomics.load(typedArray, index) → value ──────────────────────
    let atomics_load_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index_val: i64| -> i64 {
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let ptr = match sab_data_ptr(&caller, sab_handle) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            let val: f64 = unsafe {
                let addr = ptr.add(off);
                match (elem_size, element_kind) {
                    (1, 0) => (&*(addr as *const std::sync::atomic::AtomicI8)).load(Ordering::SeqCst) as f64,
                    (1, 1) | (1, 2) => (&*(addr as *const std::sync::atomic::AtomicU8)).load(Ordering::SeqCst) as f64,
                    (2, 0) => (&*(addr as *const std::sync::atomic::AtomicI16)).load(Ordering::SeqCst) as f64,
                    (2, 1) => (&*(addr as *const std::sync::atomic::AtomicU16)).load(Ordering::SeqCst) as f64,
                    (4, 0) => (&*(addr as *const std::sync::atomic::AtomicI32)).load(Ordering::SeqCst) as f64,
                    (4, 1) => (&*(addr as *const std::sync::atomic::AtomicU32)).load(Ordering::SeqCst) as f64,
                    (4, 3) => f32::from_bits((&*(addr as *const std::sync::atomic::AtomicU32)).load(Ordering::SeqCst)) as f64,
                    (8, 3) => f64::from_bits((&*(addr as *const std::sync::atomic::AtomicU64)).load(Ordering::SeqCst)),
                    _ => return value::encode_undefined(),
                }
            };
            value::encode_f64(val)
        },
    );

    // ── Atomics.store(typedArray, index, value) → value ──────────────
    let atomics_store_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index_val: i64, value_val: i64| -> i64 {
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let f_raw = value::decode_f64(value_val);
            let ptr = match sab_data_mut_ptr(&mut caller, sab_handle) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            unsafe {
                let addr = ptr.add(off);
                match (elem_size, element_kind) {
                    (1, 0) => (&*(addr as *mut std::sync::atomic::AtomicI8)).store(f_raw as i8, Ordering::SeqCst),
                    (1, 1) | (1, 2) => (&*(addr as *mut std::sync::atomic::AtomicU8)).store(f_raw as u8, Ordering::SeqCst),
                    (2, 0) => (&*(addr as *mut std::sync::atomic::AtomicI16)).store(f_raw as i16, Ordering::SeqCst),
                    (2, 1) => (&*(addr as *mut std::sync::atomic::AtomicU16)).store(f_raw as u16, Ordering::SeqCst),
                    (4, 0) => (&*(addr as *mut std::sync::atomic::AtomicI32)).store(f_raw as i32, Ordering::SeqCst),
                    (4, 1) => (&*(addr as *mut std::sync::atomic::AtomicU32)).store(f_raw as u32, Ordering::SeqCst),
                    (4, 3) => (&*(addr as *mut std::sync::atomic::AtomicU32)).store((f_raw as f32).to_bits(), Ordering::SeqCst),
                    (8, 3) => (&*(addr as *mut std::sync::atomic::AtomicU64)).store(f_raw.to_bits(), Ordering::SeqCst),
                    _ => {}
                }
            }
            value_val
        },
    );

    // ── Atomics.add(typedArray, index, value) → old value ────────────
    let atomics_add_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index_val: i64, value_val: i64| -> i64 {
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let f_raw = value::decode_f64(value_val);
            let ptr = match sab_data_mut_ptr(&mut caller, sab_handle) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            let old: f64 = unsafe {
                let addr = ptr.add(off);
                match (elem_size, element_kind) {
                    (1, 0) => (&*(addr as *const std::sync::atomic::AtomicI8)).fetch_add(f_raw as i8, Ordering::SeqCst) as f64,
                    (1, 1) => (&*(addr as *const std::sync::atomic::AtomicU8)).fetch_add(f_raw as u8, Ordering::SeqCst) as f64,
                    (2, 0) => (&*(addr as *const std::sync::atomic::AtomicI16)).fetch_add(f_raw as i16, Ordering::SeqCst) as f64,
                    (2, 1) => (&*(addr as *const std::sync::atomic::AtomicU16)).fetch_add(f_raw as u16, Ordering::SeqCst) as f64,
                    (4, 0) => (&*(addr as *const std::sync::atomic::AtomicI32)).fetch_add(f_raw as i32, Ordering::SeqCst) as f64,
                    (4, 1) => (&*(addr as *const std::sync::atomic::AtomicU32)).fetch_add(f_raw as u32, Ordering::SeqCst) as f64,
                    _ => return value::encode_undefined(),
                }
            };
            value::encode_f64(old)
        },
    );

    // ── Atomics.sub(typedArray, index, value) → old value ────────────
    let atomics_sub_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index_val: i64, value_val: i64| -> i64 {
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let f_raw = value::decode_f64(value_val);
            let ptr = match sab_data_mut_ptr(&mut caller, sab_handle) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            let old: f64 = unsafe {
                let addr = ptr.add(off);
                match (elem_size, element_kind) {
                    (1, 0) => (&*(addr as *const std::sync::atomic::AtomicI8)).fetch_sub(f_raw as i8, Ordering::SeqCst) as f64,
                    (1, 1) => (&*(addr as *const std::sync::atomic::AtomicU8)).fetch_sub(f_raw as u8, Ordering::SeqCst) as f64,
                    (2, 0) => (&*(addr as *const std::sync::atomic::AtomicI16)).fetch_sub(f_raw as i16, Ordering::SeqCst) as f64,
                    (2, 1) => (&*(addr as *const std::sync::atomic::AtomicU16)).fetch_sub(f_raw as u16, Ordering::SeqCst) as f64,
                    (4, 0) => (&*(addr as *const std::sync::atomic::AtomicI32)).fetch_sub(f_raw as i32, Ordering::SeqCst) as f64,
                    (4, 1) => (&*(addr as *const std::sync::atomic::AtomicU32)).fetch_sub(f_raw as u32, Ordering::SeqCst) as f64,
                    _ => return value::encode_undefined(),
                }
            };
            value::encode_f64(old)
        },
    );

    // ── Atomics.and(typedArray, index, value) → old value ────────────
    let atomics_and_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index_val: i64, value_val: i64| -> i64 {
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let f_raw = value::decode_f64(value_val);
            let ptr = match sab_data_mut_ptr(&mut caller, sab_handle) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            let old: f64 = unsafe {
                let addr = ptr.add(off);
                match (elem_size, element_kind) {
                    (1, 0) => (&*(addr as *const std::sync::atomic::AtomicI8)).fetch_and(f_raw as i8, Ordering::SeqCst) as f64,
                    (1, 1) => (&*(addr as *const std::sync::atomic::AtomicU8)).fetch_and(f_raw as u8, Ordering::SeqCst) as f64,
                    (2, 0) => (&*(addr as *const std::sync::atomic::AtomicI16)).fetch_and(f_raw as i16, Ordering::SeqCst) as f64,
                    (2, 1) => (&*(addr as *const std::sync::atomic::AtomicU16)).fetch_and(f_raw as u16, Ordering::SeqCst) as f64,
                    (4, 0) => (&*(addr as *const std::sync::atomic::AtomicI32)).fetch_and(f_raw as i32, Ordering::SeqCst) as f64,
                    (4, 1) => (&*(addr as *const std::sync::atomic::AtomicU32)).fetch_and(f_raw as u32, Ordering::SeqCst) as f64,
                    _ => return value::encode_undefined(),
                }
            };
            value::encode_f64(old)
        },
    );

    // ── Atomics.or(typedArray, index, value) → old value ─────────────
    let atomics_or_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index_val: i64, value_val: i64| -> i64 {
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let f_raw = value::decode_f64(value_val);
            let ptr = match sab_data_mut_ptr(&mut caller, sab_handle) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            let old: f64 = unsafe {
                let addr = ptr.add(off);
                match (elem_size, element_kind) {
                    (1, 0) => (&*(addr as *const std::sync::atomic::AtomicI8)).fetch_or(f_raw as i8, Ordering::SeqCst) as f64,
                    (1, 1) => (&*(addr as *const std::sync::atomic::AtomicU8)).fetch_or(f_raw as u8, Ordering::SeqCst) as f64,
                    (2, 0) => (&*(addr as *const std::sync::atomic::AtomicI16)).fetch_or(f_raw as i16, Ordering::SeqCst) as f64,
                    (2, 1) => (&*(addr as *const std::sync::atomic::AtomicU16)).fetch_or(f_raw as u16, Ordering::SeqCst) as f64,
                    (4, 0) => (&*(addr as *const std::sync::atomic::AtomicI32)).fetch_or(f_raw as i32, Ordering::SeqCst) as f64,
                    (4, 1) => (&*(addr as *const std::sync::atomic::AtomicU32)).fetch_or(f_raw as u32, Ordering::SeqCst) as f64,
                    _ => return value::encode_undefined(),
                }
            };
            value::encode_f64(old)
        },
    );

    // ── Atomics.xor(typedArray, index, value) → old value ────────────
    let atomics_xor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index_val: i64, value_val: i64| -> i64 {
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let f_raw = value::decode_f64(value_val);
            let ptr = match sab_data_mut_ptr(&mut caller, sab_handle) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            let old: f64 = unsafe {
                let addr = ptr.add(off);
                match (elem_size, element_kind) {
                    (1, 0) => (&*(addr as *const std::sync::atomic::AtomicI8)).fetch_xor(f_raw as i8, Ordering::SeqCst) as f64,
                    (1, 1) => (&*(addr as *const std::sync::atomic::AtomicU8)).fetch_xor(f_raw as u8, Ordering::SeqCst) as f64,
                    (2, 0) => (&*(addr as *const std::sync::atomic::AtomicI16)).fetch_xor(f_raw as i16, Ordering::SeqCst) as f64,
                    (2, 1) => (&*(addr as *const std::sync::atomic::AtomicU16)).fetch_xor(f_raw as u16, Ordering::SeqCst) as f64,
                    (4, 0) => (&*(addr as *const std::sync::atomic::AtomicI32)).fetch_xor(f_raw as i32, Ordering::SeqCst) as f64,
                    (4, 1) => (&*(addr as *const std::sync::atomic::AtomicU32)).fetch_xor(f_raw as u32, Ordering::SeqCst) as f64,
                    _ => return value::encode_undefined(),
                }
            };
            value::encode_f64(old)
        },
    );

    // ── Atomics.exchange(typedArray, index, value) → old value ───────
    let atomics_exchange_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index_val: i64, value_val: i64| -> i64 {
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let f_raw = value::decode_f64(value_val);
            let ptr = match sab_data_mut_ptr(&mut caller, sab_handle) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            let old: f64 = unsafe {
                let addr = ptr.add(off);
                match (elem_size, element_kind) {
                    (1, 0) => (&*(addr as *const std::sync::atomic::AtomicI8)).swap(f_raw as i8, Ordering::SeqCst) as f64,
                    (1, 1) => (&*(addr as *const std::sync::atomic::AtomicU8)).swap(f_raw as u8, Ordering::SeqCst) as f64,
                    (2, 0) => (&*(addr as *const std::sync::atomic::AtomicI16)).swap(f_raw as i16, Ordering::SeqCst) as f64,
                    (2, 1) => (&*(addr as *const std::sync::atomic::AtomicU16)).swap(f_raw as u16, Ordering::SeqCst) as f64,
                    (4, 0) => (&*(addr as *const std::sync::atomic::AtomicI32)).swap(f_raw as i32, Ordering::SeqCst) as f64,
                    (4, 1) => (&*(addr as *const std::sync::atomic::AtomicU32)).swap(f_raw as u32, Ordering::SeqCst) as f64,
                    _ => return value::encode_undefined(),
                }
            };
            value::encode_f64(old)
        },
    );

    // ── Atomics.compareExchange(typedArray, index, expected, replacement) → old value ─
    let atomics_compare_exchange_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index_val: i64, expected_val: i64, replacement_val: i64| -> i64 {
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            let index = value::decode_f64(index_val) as u32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let expected = value::decode_f64(expected_val);
            let replacement = value::decode_f64(replacement_val);
            let ptr = match sab_data_mut_ptr(&mut caller, sab_handle) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            let old: f64 = unsafe {
                let addr = ptr.add(off);
                match (elem_size, element_kind) {
                    (1, 0) => {
                        let atom = &*(addr as *const std::sync::atomic::AtomicI8);
                        match atom.compare_exchange(expected as i8, replacement as i8, Ordering::SeqCst, Ordering::SeqCst) {
                            Ok(v) | Err(v) => v as f64,
                        }
                    }
                    (1, 1) => {
                        let atom = &*(addr as *const std::sync::atomic::AtomicU8);
                        match atom.compare_exchange(expected as u8, replacement as u8, Ordering::SeqCst, Ordering::SeqCst) {
                            Ok(v) | Err(v) => v as f64,
                        }
                    }
                    (2, 0) => {
                        let atom = &*(addr as *const std::sync::atomic::AtomicI16);
                        match atom.compare_exchange(expected as i16, replacement as i16, Ordering::SeqCst, Ordering::SeqCst) {
                            Ok(v) | Err(v) => v as f64,
                        }
                    }
                    (2, 1) => {
                        let atom = &*(addr as *const std::sync::atomic::AtomicU16);
                        match atom.compare_exchange(expected as u16, replacement as u16, Ordering::SeqCst, Ordering::SeqCst) {
                            Ok(v) | Err(v) => v as f64,
                        }
                    }
                    (4, 0) => {
                        let atom = &*(addr as *const std::sync::atomic::AtomicI32);
                        match atom.compare_exchange(expected as i32, replacement as i32, Ordering::SeqCst, Ordering::SeqCst) {
                            Ok(v) | Err(v) => v as f64,
                        }
                    }
                    (4, 1) => {
                        let atom = &*(addr as *const std::sync::atomic::AtomicU32);
                        match atom.compare_exchange(expected as u32, replacement as u32, Ordering::SeqCst, Ordering::SeqCst) {
                            Ok(v) | Err(v) => v as f64,
                        }
                    }
                    _ => return value::encode_undefined(),
                }
            };
            value::encode_f64(old)
        },
    );

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
            if result { value::encode_f64(1.0) } else { value::encode_f64(0.0) }
        },
    );

    // ── Atomics.wait(typedArray, index, expected, timeout) → string ──
    // Only valid on Int32Array. Returns "not-equal", "ok", or "timed-out".
    // Simplified: without real threading, always "timed-out" unless value mismatch.
    let atomics_wait_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index_val: i64, expected_val: i64, _timeout_val: i64| -> i64 {
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            // Per spec: only Int32
            if elem_size != 4 || element_kind != 0 {
                return value::encode_undefined();
            }
            let index = value::decode_f64(index_val) as u32;
            let expected = value::decode_f64(expected_val) as i32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let ptr = match sab_data_mut_ptr(&mut caller, sab_handle) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            let current = unsafe {
                let addr = ptr.add(off);
                (&*(addr as *const std::sync::atomic::AtomicI32)).load(Ordering::SeqCst)
            };
            if current != expected {
                return store_runtime_string(&mut caller, "not-equal".to_string());
            }
            // Without real threading, immediately timed out.
            store_runtime_string(&mut caller, "timed-out".to_string())
        },
    );

    // ── Atomics.notify(typedArray, index, count) → i64 ───────────────
    let atomics_notify_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, _index_val: i64, _count_val: i64| -> i64 {
            let (_byte_offset, elem_size, element_kind, _sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            // Per spec: only Int32
            if elem_size != 4 || element_kind != 0 {
                return value::encode_undefined();
            }
            // Without real threading, no waiters to wake.
            value::encode_f64(0.0)
        },
    );

    // ── Atomics.waitAsync(typedArray, index, value, timeout) → object ─
    let atomics_wait_async_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index_val: i64, value_val: i64, _timeout_val: i64| -> i64 {
            let (byte_offset, elem_size, element_kind, sab_handle) =
                match validate_ta_for_atomics(&mut caller, this_val) {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            // Per spec: only Int32
            if elem_size != 4 || element_kind != 0 {
                return value::encode_undefined();
            }
            let index = value::decode_f64(index_val) as u32;
            let expected = value::decode_f64(value_val) as i32;
            let off = byte_offset + (index as usize) * (elem_size as usize);
            let ptr = match sab_data_mut_ptr(&mut caller, sab_handle) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            let current = unsafe {
                let addr = ptr.add(off);
                (&*(addr as *const std::sync::atomic::AtomicI32)).load(Ordering::SeqCst)
            };
            // Return { async: false, value: "not-equal" } or { async: false, value: "timed-out" }
            let result = { let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv"); alloc_host_object(&mut caller, &_wjsm_env, 4) };
            let _ = define_host_data_property_from_caller(
                &mut caller,
                result,
                "async",
                value::encode_f64(0.0), // false
            );
            let status = if current != expected {
                "not-equal"
            } else {
                "timed-out"
            };
            let status_str = store_runtime_string(&mut caller, status.to_string());
            let _ = define_host_data_property_from_caller(
                &mut caller,
                result,
                "value",
                status_str,
            );
            result
        },
    );
    // ── SharedArrayBuffer stubs (indices 361-364 in WASM module) ──
    let sab_constructor_stub = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, byte_length_val: i64| -> i64 {
            let byte_length = value::decode_f64(byte_length_val) as u64;
            let shared = match caller.data().shared_state.clone() {
                Some(s) => s,
                None => return value::encode_undefined(),
            };
            let entry = SharedArrayBufferEntry {
                data: Arc::new(RwLock::new(vec![0u8; byte_length as usize])),
                byte_length,
            };
            let mut table = shared.sab_table.lock().unwrap();
            table.push(entry);
            let handle = (table.len() - 1) as u32;
            let obj = { let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv"); alloc_host_object(&mut caller, &_wjsm_env, 4) };
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__sharedarraybuffer_handle__", value::encode_f64(handle as f64));
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteLength", value::encode_f64(byte_length as f64));
            obj
        },
    );
    let sab_byte_length_stub = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if !value::is_object(this_val) { return value::encode_undefined(); }
            let obj_ptr = match resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            match read_object_property_by_name(&mut caller, obj_ptr, "byteLength") {
                Some(v) => v,
                None => value::encode_undefined(),
            }
        },
    );
    let sab_slice_stub = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, this_val: i64, _begin: i64, _end: i64| -> i64 {
            this_val
        },
    );
    let sab_species_stub = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            this_val
        },
    );

    vec![
        sab_constructor_stub.into(),       // 361
        sab_byte_length_stub.into(),       // 362
        sab_slice_stub.into(),             // 363
        sab_species_stub.into(),           // 364
        atomics_load_fn.into(),            // 365
        atomics_store_fn.into(),           // 366
        atomics_add_fn.into(),             // 367
        atomics_sub_fn.into(),             // 368
        atomics_and_fn.into(),             // 369
        atomics_or_fn.into(),              // 370
        atomics_xor_fn.into(),             // 371
        atomics_exchange_fn.into(),        // 372
        atomics_compare_exchange_fn.into(),// 373
        atomics_is_lock_free_fn.into(),    // 374
        atomics_wait_fn.into(),            // 375
        atomics_notify_fn.into(),          // 376
        atomics_wait_async_fn.into(),      // 377
    ]
}
