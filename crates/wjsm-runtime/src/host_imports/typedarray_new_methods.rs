{
    // ── TypedArray 辅助函数 ─────────────────────────────────────────
    /// 解析 TypedArray 的 this_val，返回 (buffer_handle, byte_offset, length, element_size, element_kind)
    fn ta_resolve(
        caller: &mut Caller<'_, RuntimeState>,
        this_val: i64,
    ) -> Option<(usize, usize, u32, u8, u8)> {
        if !value::is_object(this_val) {
            return None;
        }
        let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize)?;
        let h = read_object_property_by_name(caller, obj_ptr, "__typedarray_handle__")?;
        let handle = value::decode_f64(h) as usize;
        let table = caller.data().typedarray_table.lock().ok()?;
        let entry = table.get(handle)?;
        Some((entry.buffer_handle as usize, entry.byte_offset as usize, entry.length, entry.element_size, entry.element_kind))
    }

    /// 读取 TypedArray 第 index 个元素，返回 NaN-boxed f64 值。
    /// 根据 element_kind 区分整数/无符号/浮点读写。
    fn ta_read(
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
            _ => return None,
        };
        Some(value::encode_f64(val))
    }

    /// 写入 TypedArray 第 index 个元素，根据 element_kind 采用对应的整数/浮点编码。
    fn ta_write(
        caller: &mut Caller<'_, RuntimeState>,
        buf_handle: usize,
        byte_offset: usize,
        elem_size: u8,
        element_kind: u8,
        index: u32,
        val: i64,
    ) -> Option<()> {
        let f_raw = value::decode_f64(val);
        let mut table = caller.data().arraybuffer_table.lock().ok()?;
        let entry = table.get_mut(buf_handle)?;
        let off = byte_offset + (index as usize) * (elem_size as usize);
        if off + (elem_size as usize) > entry.data.len() {
            return None;
        }
        match (elem_size, element_kind) {
            // Int8
            (1, 0) => { entry.data[off] = f_raw as i8 as u8; }
            // Uint8 / Uint8Clamped: clamp to 0..255
            (1, 1) => { entry.data[off] = f_raw as u8; }
            (1, 2) => { entry.data[off] = f_raw.round().clamp(0.0, 255.0) as u8; }
            // Int16
            (2, 0) => { entry.data[off..off + 2].copy_from_slice(&(f_raw as i16).to_le_bytes()); }
            // Uint16
            (2, 1) => { entry.data[off..off + 2].copy_from_slice(&(f_raw as u16).to_le_bytes()); }
            // Int32
            (4, 0) => { entry.data[off..off + 4].copy_from_slice(&(f_raw as i32).to_le_bytes()); }
            // Uint32
            (4, 1) => { entry.data[off..off + 4].copy_from_slice(&(f_raw as u32).to_le_bytes()); }
            // Float32
            (4, 3) => { entry.data[off..off + 4].copy_from_slice(&(f_raw as f32).to_le_bytes()); }
            // Float64
            (8, 3) => { entry.data[off..off + 8].copy_from_slice(&f_raw.to_le_bytes()); }
            _ => return None,
        }
        Some(())
    }

    // ── typedarray_proto_fill (Type 17, 4-arg: this, value, start, end) ──
    let typedarray_proto_fill_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_val: i64,
         value: i64,
         start_raw: i64,
         end_raw: i64|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
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
                ta_write(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i, value);
            }
            this_val
        },
    );

    // ── typedarray_proto_reverse (Type 3, 1-arg: this) ──
    let typedarray_proto_reverse_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return this_val,
            };
            for i in 0..length / 2 {
                let a = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                let b = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, length - 1 - i)
                    .unwrap_or(value::encode_undefined());
                ta_write(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i, b);
                ta_write(&mut caller, buf_handle, byte_offset, elem_size, element_kind, length - 1 - i, a);
            }
            this_val
        },
    );

    // ── typedarray_proto_index_of (Type 16, 3-arg: this, searchElement, fromIndex) ──
    let typedarray_proto_index_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, search_element: i64, from_index: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
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
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                if same_value_zero(elem, search_element) {
                    return value::encode_f64(i as f64);
                }
            }
            value::encode_f64(-1.0)
        },
    );

    // ── typedarray_proto_last_index_of (Type 16, 3-arg: this, searchElement, fromIndex) ──
    let typedarray_proto_last_index_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, search_element: i64, from_index: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
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
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                if same_value_zero(elem, search_element) {
                    return value::encode_f64(i as f64);
                }
            }
            value::encode_f64(-1.0)
        },
    );

    // ── typedarray_proto_includes (Type 16, 3-arg: this, searchElement, fromIndex) ──
    let typedarray_proto_includes_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, search_element: i64, from_index: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
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
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                if same_value_zero(elem, search_element) {
                    return value::encode_bool(true);
                }
            }
            value::encode_bool(false)
        },
    );

    // ── typedarray_proto_join (Type 2, 2-arg: this, separator) ──
    let typedarray_proto_join_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, separator: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
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
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                parts.push(render_value(&mut caller, elem).unwrap_or_else(|_| "".to_string()));
            }
            store_runtime_string(&caller, parts.join(&sep))
        },
    );

    // ── typedarray_proto_to_string (Type 3, 1-arg: this) ──
    let typedarray_proto_to_string_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return store_runtime_string(&caller, String::new()),
            };
            let mut parts = Vec::new();
            for i in 0..length {
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                parts.push(render_value(&mut caller, elem).unwrap_or_else(|_| "".to_string()));
            }
            store_runtime_string(&caller, parts.join(","))
        },
    );

    // ── typedarray_proto_copy_within (Type 16, 3-arg: this, target, start, end via shadow stack) ──
    // Note: backend passes 3 WASM args (this, target, start) but end comes via shadow stack
    let typedarray_proto_copy_within_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, target_val: i64, start_val: i64, end_val: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return this_val,
            };
            let target = {
                let f = value::decode_f64(target_val);
                if f < 0.0 {                    (length as i32 + (f as i32)).max(0) as u32
            } else {
                (f as u32).min(length) }
            };
            let start = {
                let f = value::decode_f64(start_val);
                if f < 0.0 {                    (length as i32 + (f as i32)).max(0) as u32
            } else {
                (f as u32).min(length) }
            };
            let end = {
                let f = value::decode_f64(end_val);
                if f < 0.0 {                    (length as i32 + (f as i32)).max(0) as u32
            } else {
                (f as u32).min(length) }
            };
            let count = end.saturating_sub(start);
            let count = count.min(length.saturating_sub(target));
            if count == 0 {
                return this_val;
            }
            if target < start {
                for i in 0..count {
                    let elem =                        ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, start + i)
                        .unwrap_or(value::encode_undefined());
                    ta_write(&mut caller, buf_handle, byte_offset, elem_size, element_kind, target + i, elem);
                }
            } else {
                for i in (0..count).rev() {
                    let elem =                        ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, start + i)
                        .unwrap_or(value::encode_undefined());
                    ta_write(&mut caller, buf_handle, byte_offset, elem_size, element_kind, target + i, elem);
                }
            }
            this_val
        },
    );

    // ── typedarray_proto_at (Type 2, 2-arg: this, index) ──
    let typedarray_proto_at_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
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
            ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, idx as u32)
                .unwrap_or(value::encode_undefined())
        },
    );

    // ── typedarray_proto_for_each (Type 12, 影子栈: this, callback, thisArg) ──
    let typedarray_proto_for_each_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return value::encode_undefined(),
            };
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            for i in 0..length {
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                if call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]).is_err() {
                    return value::encode_undefined();
                }
            }
            value::encode_undefined()
        },
    );

    // ── typedarray_proto_map (Type 12, 影子栈: this, callback, thisArg) ──
    let typedarray_proto_map_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return value::encode_undefined(),
            };
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            // map 返回新的 Array（非 TypedArray）
            let new_arr = alloc_array(&mut caller, length);
            let Some(arr_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for i in 0..length {
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                let mapped = match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]) {
                    Ok(v) => v,
                    Err(_) => return value::encode_undefined(),
                };
                write_array_elem(&mut caller, arr_ptr, i, mapped);
            }
            write_array_length(&mut caller, arr_ptr, length);
            new_arr
        },
    );

    // ── typedarray_proto_filter (Type 12, 影子栈: this, callback, thisArg) ──
    let typedarray_proto_filter_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return value::encode_undefined(),
            };
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            let mut results = Vec::new();
            for i in 0..length {
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                let keep = match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]) {
                    Ok(v) => value::is_truthy(v),
                    Err(_) => return value::encode_undefined(),
                };
                if keep {
                    results.push(elem);
                }
            }
            let new_arr = alloc_array(&mut caller, results.len() as u32);
            let Some(arr_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for (j, elem) in results.iter().enumerate() {
                write_array_elem(&mut caller, arr_ptr, j as u32, *elem);
            }
            write_array_length(&mut caller, arr_ptr, results.len() as u32);
            new_arr
        },
    );

    // ── typedarray_proto_reduce (Type 12, 影子栈: this, callback, initialValue) ──
    let typedarray_proto_reduce_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return value::encode_undefined(),
            };
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let has_init = args_count > 1;
            let init = if has_init {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            if length == 0 && !has_init {
                return value::encode_undefined();
            }
            let mut acc = if has_init {
                init
            } else {
                ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, 0)
                    .unwrap_or(value::encode_undefined())
            };
            let start = if has_init { 0 } else { 1 };
            for i in start..length {
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                acc = match call_wasm_callback(&mut caller, cb, value::encode_undefined(), &[acc, elem, idx_val, this_val]) {
                    Ok(v) => v,
                    Err(_) => return value::encode_undefined(),
                };
            }
            acc
        },
    );

    // ── typedarray_proto_reduce_right (Type 12, 影子栈: this, callback, initialValue) ──
    let typedarray_proto_reduce_right_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return value::encode_undefined(),
            };
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let has_init = args_count > 1;
            let init = if has_init {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            if length == 0 && !has_init {
                return value::encode_undefined();
            }
            let mut acc = if has_init {
                init
            } else {
                ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, length - 1)
                    .unwrap_or(value::encode_undefined())
            };
            let end = if has_init { length as i32 - 1 } else { length as i32 - 2 };
            for i in (0..=end as u32).rev() {
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                acc = match call_wasm_callback(&mut caller, cb, value::encode_undefined(), &[acc, elem, idx_val, this_val]) {
                    Ok(v) => v,
                    Err(_) => return value::encode_undefined(),
                };
            }
            acc
        },
    );

    // ── typedarray_proto_find (Type 12, 影子栈: this, callback, thisArg) ──
    let typedarray_proto_find_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return value::encode_undefined(),
            };
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            for i in 0..length {
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                let found = match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]) {
                    Ok(v) => value::is_truthy(v),
                    Err(_) => return value::encode_undefined(),
                };
                if found {
                    return elem;
                }
            }
            value::encode_undefined()
        },
    );

    // ── typedarray_proto_find_index (Type 12, 影子栈: this, callback, thisArg) ──
    let typedarray_proto_find_index_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return value::encode_f64(-1.0),
            };
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_f64(-1.0);
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            for i in 0..length {
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                let found = match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]) {
                    Ok(v) => value::is_truthy(v),
                    Err(_) => return value::encode_f64(-1.0),
                };
                if found {
                    return value::encode_f64(i as f64);
                }
            }
            value::encode_f64(-1.0)
        },
    );

    // ── typedarray_proto_some (Type 12, 影子栈: this, callback, thisArg) ──
    let typedarray_proto_some_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return value::encode_bool(false),
            };
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_bool(false);
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            for i in 0..length {
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                let ok = match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]) {
                    Ok(v) => value::is_truthy(v),
                    Err(_) => return value::encode_bool(false),
                };
                if ok {
                    return value::encode_bool(true);
                }
            }
            value::encode_bool(false)
        },
    );

    // ── typedarray_proto_every (Type 12, 影子栈: this, callback, thisArg) ──
    let typedarray_proto_every_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return value::encode_bool(true),
            };
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_bool(true);
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            for i in 0..length {
                let elem = ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i)
                    .unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                let ok = match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]) {
                    Ok(v) => value::is_truthy(v),
                    Err(_) => return value::encode_bool(false),
                };
                if !ok {
                    return value::encode_bool(false);
                }
            }
            value::encode_bool(true)
        },
    );

    // ── typedarray_proto_sort (Type 12, 影子栈: this, compareFn) ──
    let typedarray_proto_sort_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let (buf_handle, byte_offset, length, elem_size, element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return this_val,
            };
            if length <= 1 {
                return this_val;
            }
            // 将所有元素读到 Vec
            let mut elems: Vec<i64> = (0..length)
                .map(|i| ta_read(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i).unwrap_or(value::encode_undefined()))
                .collect();
            if args_count > 0 && value::is_callable(read_shadow_arg(&mut caller, args_base, 0)) {
                let cmp = read_shadow_arg(&mut caller, args_base, 0);
                elems.sort_by(|a, b| {
                    let result = call_wasm_callback(&mut caller, cmp, value::encode_undefined(), &[*a, *b])
                        .unwrap_or(value::encode_f64(0.0));
                    let v = f64::from_bits(result as u64);
                    if v > 0.0 { std::cmp::Ordering::Greater }
                    else if v < 0.0 { std::cmp::Ordering::Less }
                    else { std::cmp::Ordering::Equal }
                });
            } else {
                elems.sort_by(|a, b| {
                    let sa = render_value(&mut caller, *a).unwrap_or_default();
                    let sb = render_value(&mut caller, *b).unwrap_or_default();
                    sa.cmp(&sb)
                });
            }
            // 写回
            for (i, &elem) in elems.iter().enumerate() {
                ta_write(&mut caller, buf_handle, byte_offset, elem_size, element_kind, i as u32, elem);
            }
            this_val
        },
    );

    // ── typedarray_proto_entries (Type 3, 1-arg: this) ──
    let typedarray_proto_entries_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, _element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return value::encode_undefined(),
            };
            let handle;
            {
                let mut iter_table = caller.data().iterators.lock().expect("iterators mutex");
                handle = iter_table.len() as u32;
                iter_table.push(IteratorState::ArrayIter {
                    ptr: buf_handle | (byte_offset << 20) | ((elem_size as usize) << 28),
                    index: 0,
                    length,
                });
            }
            let obj = alloc_host_object_from_caller(&mut caller, 2);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iter_handle__", handle_val);
            value::encode_handle(value::TAG_ITERATOR, handle)
        },
    );

    // ── typedarray_proto_keys (Type 3, 1-arg: this) ──
    let typedarray_proto_keys_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, _element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return value::encode_undefined(),
            };
            let handle;
            {
                let mut iter_table = caller.data().iterators.lock().expect("iterators mutex");
                handle = iter_table.len() as u32;
                iter_table.push(IteratorState::ArrayIter {
                    ptr: buf_handle | (byte_offset << 20) | ((elem_size as usize) << 28),
                    index: 0,
                    length,
                });
            }
            let obj = alloc_host_object_from_caller(&mut caller, 2);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iter_handle__", handle_val);
            value::encode_handle(value::TAG_ITERATOR, handle)
        },
    );

    // ── typedarray_proto_values (Type 3, 1-arg: this) ──
    let typedarray_proto_values_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let (buf_handle, byte_offset, length, elem_size, _element_kind) = match ta_resolve(&mut caller, this_val) {
                Some(v) => v,
                None => return value::encode_undefined(),
            };
            let handle;
            {
                let mut iter_table = caller.data().iterators.lock().expect("iterators mutex");
                handle = iter_table.len() as u32;
                iter_table.push(IteratorState::ArrayIter {
                    ptr: buf_handle | (byte_offset << 20) | ((elem_size as usize) << 28),
                    index: 0,
                    length,
                });
            }
            let obj = alloc_host_object_from_caller(&mut caller, 2);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iter_handle__", handle_val);
            value::encode_handle(value::TAG_ITERATOR, handle)
        },
    );

    vec![
        typedarray_proto_fill_fn.into(),              // 326
        typedarray_proto_reverse_fn.into(),           // 327
        typedarray_proto_index_of_fn.into(),          // 328
        typedarray_proto_last_index_of_fn.into(),     // 329
        typedarray_proto_includes_fn.into(),          // 330
        typedarray_proto_join_fn.into(),              // 331
        typedarray_proto_to_string_fn.into(),         // 332
        typedarray_proto_copy_within_fn.into(),       // 333
        typedarray_proto_at_fn.into(),                // 334
        typedarray_proto_for_each_fn.into(),          // 335
        typedarray_proto_map_fn.into(),               // 336
        typedarray_proto_filter_fn.into(),            // 337
        typedarray_proto_reduce_fn.into(),            // 338
        typedarray_proto_reduce_right_fn.into(),      // 339
        typedarray_proto_find_fn.into(),              // 340
        typedarray_proto_find_index_fn.into(),        // 341
        typedarray_proto_some_fn.into(),              // 342
        typedarray_proto_every_fn.into(),             // 343
        typedarray_proto_sort_fn.into(),              // 344
        typedarray_proto_entries_fn.into(),           // 345
        typedarray_proto_keys_fn.into(),              // 346
        typedarray_proto_values_fn.into(),            // 347
    ]
}
