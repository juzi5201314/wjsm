{
    let map_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let handle;
            {
                let mut table = caller.data().map_table.lock().expect("map table mutex");
                table.push(MapEntry { keys: Vec::new(), values: Vec::new() });
                handle = table.len() as u32 - 1;
            }
            let (set_fn, get_fn, has_fn, delete_fn, clear_fn, size_fn, for_each_fn, keys_fn, values_fn, entries_fn) = {
                let state = caller.data();
                (
                    create_map_set_method(state, MapSetMethodKind::MapSet),
                    create_map_set_method(state, MapSetMethodKind::MapGet),
                    create_map_set_method(state, MapSetMethodKind::Has),
                    create_map_set_method(state, MapSetMethodKind::Delete),
                    create_map_set_method(state, MapSetMethodKind::Clear),
                    create_map_set_method(state, MapSetMethodKind::Size),
                    create_map_set_method(state, MapSetMethodKind::ForEach),
                    create_map_set_method(state, MapSetMethodKind::Keys),
                    create_map_set_method(state, MapSetMethodKind::Values),
                    create_map_set_method(state, MapSetMethodKind::Entries),
                )
            };
            let obj = alloc_host_object_from_caller(&mut caller, 11);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__map_handle__", handle_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "set", set_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "get", get_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "has", has_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "delete", delete_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "clear", clear_fn);
            // TODO: size should be a getter accessor property per ES spec, but current
            // architecture does not support call_indirect for host import functions.
            // Tracked as a known compliance gap — currently exposed as a method: m.size()
            let _ = define_host_data_property_from_caller(&mut caller, obj, "size", size_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "forEach", for_each_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "keys", keys_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "values", values_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "entries", entries_fn);
            obj
        },
    );

    let map_proto_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64, val: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__map_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let mut table = caller.data().map_table.lock().expect("map table mutex");
            if handle >= table.len() {
                return value::encode_undefined();
            }
            let entry = &mut table[handle];
            for i in 0..entry.keys.len() {
                if same_value_zero(entry.keys[i], key) {
                    entry.values[i] = val;
                    return this_val;
                }
            }
            entry.keys.push(key);
            entry.values.push(val);
            this_val
        },
    );

    let map_proto_get_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__map_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let table = caller.data().map_table.lock().expect("map table mutex");
            if handle >= table.len() {
                return value::encode_undefined();
            }
            let entry = &table[handle];
            for i in 0..entry.keys.len() {
                if same_value_zero(entry.keys[i], key) {
                    return entry.values[i];
                }
            }
            value::encode_undefined()
        },
    );

    // ── Set host functions ────────────────────────────────────────────
    let set_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let handle;
            {
                let mut table = caller.data().set_table.lock().expect("set table mutex");
                table.push(SetEntry { values: Vec::new() });
                handle = table.len() as u32 - 1;
            }
            let (add_fn, has_fn, delete_fn, clear_fn, size_fn, for_each_fn, keys_fn, values_fn, entries_fn) = {
                let state = caller.data();
                (
                    create_map_set_method(state, MapSetMethodKind::SetAdd),
                    create_map_set_method(state, MapSetMethodKind::Has),
                    create_map_set_method(state, MapSetMethodKind::Delete),
                    create_map_set_method(state, MapSetMethodKind::Clear),
                    create_map_set_method(state, MapSetMethodKind::Size),
                    create_map_set_method(state, MapSetMethodKind::ForEach),
                    create_map_set_method(state, MapSetMethodKind::Keys),
                    create_map_set_method(state, MapSetMethodKind::Values),
                    create_map_set_method(state, MapSetMethodKind::Entries),
                )
            };
            let obj = alloc_host_object_from_caller(&mut caller, 10);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__set_handle__", handle_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "add", add_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "has", has_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "delete", delete_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "clear", clear_fn);
            // TODO: ES spec requires `size` to be a getter accessor property, but the current
            // WASM architecture doesn't support calling NativeCallable via call_indirect.
            // Using a data property (method) as a workaround.
            let _ = define_host_data_property_from_caller(&mut caller, obj, "size", size_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "forEach", for_each_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "keys", keys_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "values", values_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "entries", entries_fn);
            obj
        },
    );

    let set_proto_add_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, val: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__set_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let mut table = caller.data().set_table.lock().expect("set table mutex");
            if handle >= table.len() {
                return value::encode_undefined();
            }
            let entry = &mut table[handle];
            for i in 0..entry.values.len() {
                if same_value_zero(entry.values[i], val) {
                    return this_val;
                }
            }
            entry.values.push(val);
            this_val
        },
    );

    // ── Map/Set shared host functions (dispatch at runtime) ──────────
    let map_set_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_bool(false);
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            if let Some(op) = obj_ptr {
                let map_handle = read_object_property_by_name(&mut caller, op, "__map_handle__");
                if let Some(mh) = map_handle {
                    let handle = value::decode_f64(mh) as usize;
                    let table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        let entry = &table[handle];
                        for i in 0..entry.keys.len() {
                            if same_value_zero(entry.keys[i], key) {
                                return value::encode_bool(true);
                            }
                        }
                    }
                    return value::encode_bool(false);
                }
                let set_handle = read_object_property_by_name(&mut caller, op, "__set_handle__");
                if let Some(sh) = set_handle {
                    let handle = value::decode_f64(sh) as usize;
                    let table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        let entry = &table[handle];
                        for i in 0..entry.values.len() {
                            if same_value_zero(entry.values[i], key) {
                                return value::encode_bool(true);
                            }
                        }
                    }
                    return value::encode_bool(false);
                }
            }
            value::encode_bool(false)
        },
    );

    let map_set_delete_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_bool(false);
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            if let Some(op) = obj_ptr {
                let map_handle = read_object_property_by_name(&mut caller, op, "__map_handle__");
                if let Some(mh) = map_handle {
                    let handle = value::decode_f64(mh) as usize;
                    let mut table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        let entry = &mut table[handle];
                        for i in 0..entry.keys.len() {
                            if same_value_zero(entry.keys[i], key) {
                                entry.keys.remove(i);
                                entry.values.remove(i);
                                return value::encode_bool(true);
                            }
                        }
                    }
                    return value::encode_bool(false);
                }
                let set_handle = read_object_property_by_name(&mut caller, op, "__set_handle__");
                if let Some(sh) = set_handle {
                    let handle = value::decode_f64(sh) as usize;
                    let mut table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        let entry = &mut table[handle];
                        for i in 0..entry.values.len() {
                            if same_value_zero(entry.values[i], key) {
                                entry.values.remove(i);
                                return value::encode_bool(true);
                            }
                        }
                    }
                    return value::encode_bool(false);
                }
            }
            value::encode_bool(false)
        },
    );

    let map_set_clear_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            if let Some(op) = obj_ptr {
                let map_handle = read_object_property_by_name(&mut caller, op, "__map_handle__");
                if let Some(mh) = map_handle {
                    let handle = value::decode_f64(mh) as usize;
                    let mut table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        table[handle].keys.clear();
                        table[handle].values.clear();
                    }
                    return value::encode_undefined();
                }
                let set_handle = read_object_property_by_name(&mut caller, op, "__set_handle__");
                if let Some(sh) = set_handle {
                    let handle = value::decode_f64(sh) as usize;
                    let mut table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        table[handle].values.clear();
                    }
                    return value::encode_undefined();
                }
            }
            value::encode_undefined()
        },
    );

    let map_set_get_size_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_f64(0.0);
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            if let Some(op) = obj_ptr {
                let map_handle = read_object_property_by_name(&mut caller, op, "__map_handle__");
                if let Some(mh) = map_handle {
                    let handle = value::decode_f64(mh) as usize;
                    let table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        return value::encode_f64(table[handle].keys.len() as f64);
                    }
                    return value::encode_f64(0.0);
                }
                let set_handle = read_object_property_by_name(&mut caller, op, "__set_handle__");
                if let Some(sh) = set_handle {
                    let handle = value::decode_f64(sh) as usize;
                    let table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        return value::encode_f64(table[handle].values.len() as f64);
                    }
                    return value::encode_f64(0.0);
                }
            }
            value::encode_f64(0.0)
        },
    );

    let map_set_for_each_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64| -> i64 {
            value::encode_undefined()
        },
    );

    let map_set_keys_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64| -> i64 {
            value::encode_undefined()
        },
    );

    let map_set_values_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64| -> i64 {
            value::encode_undefined()
        },
    );

    let map_set_entries_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64| -> i64 {
            value::encode_undefined()
        },
    );

    // ── WeakMap host functions ───────────────────────────────────────────
    let weakmap_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let handle;
            {
                let mut table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
                handle = table.len() as u32;
                table.push(WeakMapEntry { map: HashMap::new() });
            }
            let (set_fn, get_fn, has_fn, delete_fn) = {
                let state = caller.data();
                (
                    create_weakmap_method(state, WeakMapMethodKind::Set),
                    create_weakmap_method(state, WeakMapMethodKind::Get),
                    create_weakmap_method(state, WeakMapMethodKind::Has),
                    create_weakmap_method(state, WeakMapMethodKind::Delete),
                )
            };
            let obj = alloc_host_object_from_caller(&mut caller, 5);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__weakmap_handle__", handle_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "set", set_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "get", get_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "has", has_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "delete", delete_fn);
            obj
        },
    );

    let weakmap_proto_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64, val: i64| -> i64 {
            if !is_object_key(key) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Invalid value used as weak map key".to_string());
                return this_val;
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakmap_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
                if handle < table.len() {
                    table[handle].map.insert(key_handle, val);
                }
            }
            this_val
        },
    );

    let weakmap_proto_get_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_undefined();
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakmap_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
            if handle < table.len()
                && let Some(&val) = table[handle].map.get(&key_handle) {
                    return val;
                }
            value::encode_undefined()
        },
    );

    let weakmap_proto_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakmap_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].map.contains_key(&key_handle));
            }
            value::encode_bool(false)
        },
    );

    let weakmap_proto_delete_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakmap_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let mut table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].map.remove(&key_handle).is_some());
            }
            value::encode_bool(false)
        },
    );

    // ── WeakSet host functions ───────────────────────────────────────────
    let weakset_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let handle;
            {
                let mut table = caller.data().weakset_table.lock().expect("weakset_table mutex");
                handle = table.len() as u32;
                table.push(WeakSetEntry { set: HashSet::new() });
            }
            let (add_fn, has_fn, delete_fn) = {
                let state = caller.data();
                (
                    create_weakset_method(state, WeakSetMethodKind::Add),
                    create_weakset_method(state, WeakSetMethodKind::Has),
                    create_weakset_method(state, WeakSetMethodKind::Delete),
                )
            };
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__weakset_handle__", handle_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "add", add_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "has", has_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "delete", delete_fn);
            obj
        },
    );

    let weakset_proto_add_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Invalid value used in weak set".to_string());
                return this_val;
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakset_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller.data().weakset_table.lock().expect("weakset_table mutex");
                if handle < table.len() {
                    table[handle].set.insert(key_handle);
                }
            }
            this_val
        },
    );

    let weakset_proto_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakset_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller.data().weakset_table.lock().expect("weakset_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].set.contains(&key_handle));
            }
            value::encode_bool(false)
        },
    );

    let weakset_proto_delete_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakset_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let mut table = caller.data().weakset_table.lock().expect("weakset_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].set.remove(&key_handle));
            }
            value::encode_bool(false)
        },
    );

    let arraybuffer_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, byte_length: i64| -> i64 {
            let len_f64 = value::decode_f64(byte_length);
            if len_f64 < 0.0 {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("RangeError: Invalid array buffer length".to_string());
                return value::encode_undefined();
            }
            let len = len_f64 as u32;
            let handle;
            {
                let mut table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                handle = table.len() as u32;
                table.push(ArrayBufferEntry { data: vec![0; len as usize] });
            }
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__arraybuffer_handle__", handle_val);
            let bl_val = value::encode_f64(len as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteLength", bl_val);
            obj
        },
    );

    let arraybuffer_proto_byte_length_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => {
                    match read_object_property_by_name(&mut caller, ptr, "byteLength") {
                        Some(v) => v,
                        None => value::encode_f64(0.0),
                    }
                }
                None => value::encode_f64(0.0),
            }
        },
    );

    let arraybuffer_proto_slice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, begin: i64, end: i64| -> i64 {
            let begin_idx = value::decode_f64(begin) as u32;
            let end_idx = value::decode_f64(end) as u32;
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let (buf_handle, buf_len) = match obj_ptr {
                Some(ptr) => {
                    let h = read_object_property_by_name(&mut caller, ptr, "__arraybuffer_handle__");
                    let bl = read_object_property_by_name(&mut caller, ptr, "byteLength");
                    match (h, bl) {
                        (Some(hv), Some(lv)) => (value::decode_f64(hv) as u32, value::decode_f64(lv) as u32),
                        _ => return value::encode_undefined(),
                    }
                }
                None => return value::encode_undefined(),
            };
            let start = begin_idx.min(buf_len);
            let stop = end_idx.min(buf_len);
            let new_len = stop.saturating_sub(start);
            let new_buf_handle;
            {
                let mut ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                new_buf_handle = ab_table.len() as u32;
                let mut new_data = vec![0u8; new_len as usize];
                if let Some(buf_entry) = ab_table.get(buf_handle as usize) {
                    new_data.copy_from_slice(&buf_entry.data[start as usize..stop as usize]);
                }
                ab_table.push(ArrayBufferEntry { data: new_data });
            }
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(new_buf_handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__arraybuffer_handle__", handle_val);
            let bl_val = value::encode_f64(new_len as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteLength", bl_val);
            obj
        },
    );

    let dataview_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, buffer: i64, byte_offset: i64, byte_length: i64| -> i64 {
            let offset = if value::is_undefined(byte_offset) { 0 } else { value::decode_f64(byte_offset) as u32 };
            let (buf_handle, buf_byte_length) = {
                let obj_ptr = resolve_handle(&mut caller, buffer);
                match obj_ptr {
                    Some(ptr) => {
                        let h = read_object_property_by_name(&mut caller, ptr, "__arraybuffer_handle__");
                        let bl = read_object_property_by_name(&mut caller, ptr, "byteLength");
                        match (h, bl) {
                            (Some(hv), Some(lv)) => (value::decode_f64(hv) as u32, value::decode_f64(lv) as u32),
                            _ => return value::encode_undefined(),
                        }
                    }
                    None => return value::encode_undefined(),
                }
            };
            let length = if value::is_undefined(byte_length) {
                buf_byte_length.saturating_sub(offset)
            } else {
                value::decode_f64(byte_length) as u32
            };
            let handle;
            {
                let mut table = caller.data().dataview_table.lock().expect("dataview_table mutex");
                handle = table.len() as u32;
                table.push(DataViewEntry { buffer_handle: buf_handle, byte_offset: offset, byte_length: length });
            }
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__dataview_handle__", handle_val);
            obj
        },
    );

    macro_rules! dataview_get_fn {
        ($name:ident, $size:expr, $conv:expr) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, this_val: i64, byte_offset: i64| -> i64 {
                    let offset = value::decode_f64(byte_offset) as u32;
                    let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
                    let dv_handle = match obj_ptr {
                        Some(ptr) => {
                            match read_object_property_by_name(&mut caller, ptr, "__dataview_handle__") {
                                Some(v) => value::decode_f64(v) as usize,
                                None => return value::encode_undefined(),
                            }
                        }
                        None => return value::encode_undefined(),
                    };
                    let (buf_handle, dv_offset, dv_length) = {
                        let dv_table = caller.data().dataview_table.lock().expect("dataview_table mutex");
                        if dv_handle < dv_table.len() {
                            let e = &dv_table[dv_handle];
                            (e.buffer_handle, e.byte_offset, e.byte_length)
                        } else { return value::encode_undefined(); }
                    };
                    let abs_offset = dv_offset as usize + offset as usize;
                    let ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                    if let Some(buf_entry) = ab_table.get(buf_handle as usize) {
                        if offset + $size as u32 > dv_length || abs_offset + $size > buf_entry.data.len() {
                            *caller.data().runtime_error.lock().expect("error mutex") =
                                Some("RangeError: Offset is outside the bounds of the DataView".to_string());
                            return value::encode_undefined();
                        }
                        let bytes = &buf_entry.data[abs_offset..abs_offset + $size];
                        return $conv(bytes);
                    }
                    value::encode_undefined()
                },
            );
        };
    }

    dataview_get_fn!(dataview_proto_get_int8_fn, 1, |bytes: &[u8]| value::encode_f64(bytes[0] as i8 as f64));
    dataview_get_fn!(dataview_proto_get_uint8_fn, 1, |bytes: &[u8]| value::encode_f64(bytes[0] as f64));
    dataview_get_fn!(dataview_proto_get_int16_fn, 2, |bytes: &[u8]| value::encode_f64(i16::from_le_bytes([bytes[0], bytes[1]]) as f64));
    dataview_get_fn!(dataview_proto_get_uint16_fn, 2, |bytes: &[u8]| value::encode_f64(u16::from_le_bytes([bytes[0], bytes[1]]) as f64));
    dataview_get_fn!(dataview_proto_get_int32_fn, 4, |bytes: &[u8]| value::encode_f64(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64));
    dataview_get_fn!(dataview_proto_get_uint32_fn, 4, |bytes: &[u8]| value::encode_f64(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64));
    dataview_get_fn!(dataview_proto_get_float32_fn, 4, |bytes: &[u8]| value::encode_f64(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64));
    dataview_get_fn!(dataview_proto_get_float64_fn, 8, |bytes: &[u8]| f64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]).to_bits() as i64);

    macro_rules! dataview_set_fn {
        ($name:ident, $size:expr, $write:expr) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, this_val: i64, byte_offset: i64, value_arg: i64| -> i64 {
                    let offset = value::decode_f64(byte_offset) as u32;
                    let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
                    let dv_handle = match obj_ptr {
                        Some(ptr) => {
                            match read_object_property_by_name(&mut caller, ptr, "__dataview_handle__") {
                                Some(v) => value::decode_f64(v) as usize,
                                None => return value::encode_undefined(),
                            }
                        }
                        None => return value::encode_undefined(),
                    };
                    let (buf_handle, dv_offset, dv_length) = {
                        let dv_table = caller.data().dataview_table.lock().expect("dataview_table mutex");
                        if dv_handle < dv_table.len() {
                            let e = &dv_table[dv_handle];
                            (e.buffer_handle, e.byte_offset, e.byte_length)
                        } else { return value::encode_undefined(); }
                    };
                    let abs_offset = dv_offset as usize + offset as usize;
                    let mut ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                    if let Some(buf_entry) = ab_table.get_mut(buf_handle as usize) {
                        if offset + $size as u32 > dv_length || abs_offset + $size > buf_entry.data.len() {
                            *caller.data().runtime_error.lock().expect("error mutex") =
                                Some("RangeError: Offset is outside the bounds of the DataView".to_string());
                            return value::encode_undefined();
                        }
                        let bytes = $write(value_arg);
                        buf_entry.data[abs_offset..abs_offset + $size].copy_from_slice(&bytes[..$size]);
                    }
                    value::encode_undefined()
                },
            );
        };
    }

    dataview_set_fn!(dataview_proto_set_int8_fn, 1, |v: i64| (value::decode_f64(v) as i8).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_uint8_fn, 1, |v: i64| (value::decode_f64(v) as u8).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_int16_fn, 2, |v: i64| (value::decode_f64(v) as i16).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_uint16_fn, 2, |v: i64| (value::decode_f64(v) as u16).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_int32_fn, 4, |v: i64| (value::decode_f64(v) as i32).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_uint32_fn, 4, |v: i64| (value::decode_f64(v) as u32).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_float32_fn, 4, |v: i64| (value::decode_f64(v) as f32).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_float64_fn, 8, |v: i64| value::decode_f64(v).to_le_bytes().to_vec());

    macro_rules! typedarray_constructor {
        ($name:ident, $size:expr) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, buffer: i64, byte_offset: i64, length: i64| -> i64 {
                    let offset = value::decode_f64(byte_offset) as u32;
                    let len = value::decode_f64(length) as u32;
                    let buf_handle = {
                        let obj_ptr = resolve_handle(&mut caller, buffer);
                        match obj_ptr {
                            Some(ptr) => {
                                let h = read_object_property_by_name(&mut caller, ptr, "__arraybuffer_handle__");
                                match h { Some(v) => value::decode_f64(v) as u32, None => return value::encode_undefined() }
                            }
                            None => return value::encode_undefined(),
                        }
                    };
                    let handle;
                    {
                        let mut table = caller.data().typedarray_table.lock().expect("typedarray_table mutex");
                        handle = table.len() as u32;
                        table.push(TypedArrayEntry { buffer_handle: buf_handle, byte_offset: offset, length: len, element_size: $size });
                    }
                    let obj = alloc_host_object_from_caller(&mut caller, 4);
                    let handle_val = value::encode_f64(handle as f64);
                    let _ = define_host_data_property_from_caller(&mut caller, obj, "__typedarray_handle__", handle_val);
                    let len_val = value::encode_f64(len as f64);
                    let _ = define_host_data_property_from_caller(&mut caller, obj, "length", len_val);
                    let bl_val = value::encode_f64((len * $size as u32) as f64);
                    let _ = define_host_data_property_from_caller(&mut caller, obj, "byteLength", bl_val);
                    let bo_val = value::encode_f64(offset as f64);
                    let _ = define_host_data_property_from_caller(&mut caller, obj, "byteOffset", bo_val);
                    obj
                },
            );
        };
    }

    typedarray_constructor!(int8array_constructor_fn, 1);
    typedarray_constructor!(uint8array_constructor_fn, 1);
    typedarray_constructor!(uint8clampedarray_constructor_fn, 1);
    typedarray_constructor!(int16array_constructor_fn, 2);
    typedarray_constructor!(uint16array_constructor_fn, 2);
    typedarray_constructor!(int32array_constructor_fn, 4);
    typedarray_constructor!(uint32array_constructor_fn, 4);
    typedarray_constructor!(float32array_constructor_fn, 4);
    typedarray_constructor!(float64array_constructor_fn, 8);

    let typedarray_proto_length_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => {
                    match read_object_property_by_name(&mut caller, ptr, "length") {
                        Some(v) => v,
                        None => value::encode_f64(0.0),
                    }
                }
                None => value::encode_f64(0.0),
            }
        },
    );

    let typedarray_proto_byte_length_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => {
                    match read_object_property_by_name(&mut caller, ptr, "byteLength") {
                        Some(v) => v,
                        None => value::encode_f64(0.0),
                    }
                }
                None => value::encode_f64(0.0),
            }
        },
    );

    let typedarray_proto_byte_offset_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => {
                    match read_object_property_by_name(&mut caller, ptr, "byteOffset") {
                        Some(v) => v,
                        None => value::encode_f64(0.0),
                    }
                }
                None => value::encode_f64(0.0),
            }
        },
    );

    let typedarray_proto_set_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64, _source: i64, _offset: i64| -> i64 {
            value::encode_undefined()
        },
    );

    let typedarray_proto_slice_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64, _begin: i64, _end: i64| -> i64 {
            value::encode_undefined()
        },
    );

    let typedarray_proto_subarray_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64, _begin: i64, _end: i64| -> i64 {
            value::encode_undefined()
        },
    );

    let _get_builtin_global_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, name_val: i64| -> i64 {
            let name = read_runtime_string(&mut caller, name_val);
            let mut native_callables = caller.data().native_callables.lock().unwrap();
            let idx = native_callables.len() as u32;
            match name.as_str() {
                "Array" => {
                    native_callables.push(NativeCallable::ArrayConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Object" => {
                    native_callables.push(NativeCallable::ObjectConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Function" => {
                    native_callables.push(NativeCallable::FunctionConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "String" => {
                    native_callables.push(NativeCallable::StringConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Boolean" => {
                    native_callables.push(NativeCallable::BooleanConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Number" => {
                    native_callables.push(NativeCallable::NumberConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Symbol" => {
                    native_callables.push(NativeCallable::SymbolConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "BigInt" => {
                    native_callables.push(NativeCallable::BigIntConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "RegExp" => {
                    native_callables.push(NativeCallable::RegExpConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Error" => {
                    native_callables.push(NativeCallable::ErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "TypeError" => {
                    native_callables.push(NativeCallable::TypeErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "RangeError" => {
                    native_callables.push(NativeCallable::RangeErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "SyntaxError" => {
                    native_callables.push(NativeCallable::SyntaxErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "ReferenceError" => {
                    native_callables.push(NativeCallable::ReferenceErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "URIError" => {
                    native_callables.push(NativeCallable::URIErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "EvalError" => {
                    native_callables.push(NativeCallable::EvalErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "AggregateError" => {
                    native_callables.push(NativeCallable::AggregateErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Map" => {
                    native_callables.push(NativeCallable::MapConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Set" => {
                    native_callables.push(NativeCallable::SetConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "WeakMap" => {
                    native_callables.push(NativeCallable::WeakMapConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "WeakSet" => {
                    native_callables.push(NativeCallable::WeakSetConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Date" => {
                    native_callables.push(NativeCallable::DateConstructorGlobal);
                    value::encode_native_callable_idx(idx)
                }
                "Promise" => {
                    native_callables.push(NativeCallable::PromiseConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "ArrayBuffer" => {
                    native_callables.push(NativeCallable::ArrayBufferConstructorGlobal);
                    value::encode_native_callable_idx(idx)
                }
                "DataView" => {
                    native_callables.push(NativeCallable::DataViewConstructorGlobal);
                    value::encode_native_callable_idx(idx)
                }
                "Int8Array" | "Uint8Array" | "Uint8ClampedArray" | "Int16Array" | "Uint16Array"
                | "Int32Array" | "Uint32Array" | "Float32Array" | "Float64Array"
                | "Float16Array" | "BigInt64Array" | "BigUint64Array" => {
                    native_callables.push(NativeCallable::TypedArrayConstructor(()));
                    value::encode_native_callable_idx(idx)
                }
                "Proxy" => {
                    native_callables.push(NativeCallable::ProxyConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Math" | "JSON" | "Reflect" | "globalThis" | "Atomics"
                | "SharedArrayBuffer" | "FinalizationRegistry" | "WeakRef"
                | "parseInt" | "parseFloat" | "isNaN" | "isFinite"
                | "decodeURI" | "decodeURIComponent" | "encodeURI" | "encodeURIComponent"
                | "Temporal" | "Intl" | "Iterator" | "AsyncIterator"
                | "$262" | "eval" | "SuppressedError" => {
                    native_callables.push(NativeCallable::StubGlobal(()));
                    value::encode_native_callable_idx(idx)
                }
                _ => value::encode_undefined(),
            }
        },
    );

    let create_global_object_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>| -> i64 {
            let obj = alloc_host_object_from_caller(&mut caller, 60);
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
                ("Date", NativeCallable::DateConstructorGlobal),
                ("Promise", NativeCallable::PromiseConstructor),
                ("ArrayBuffer", NativeCallable::ArrayBufferConstructorGlobal),
                ("DataView", NativeCallable::DataViewConstructorGlobal),
                ("Proxy", NativeCallable::ProxyConstructor),
            ];
            
            for (name, callable) in builtin_pairs {
                let mut native_callables = caller.data().native_callables.lock().unwrap();
                let idx = native_callables.len() as u32;
                native_callables.push(callable.clone());
                let val = value::encode_native_callable_idx(idx);
                drop(native_callables);
                let _ = define_host_data_property_from_caller(&mut caller, obj, name, val);
            }
            
            let _ = define_host_data_property_from_caller(&mut caller, obj, "globalThis", obj);
            
            obj
        },
    );

    let create_exception_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, thrown_value: i64| -> i64 {
            let mut errors = caller.data().error_table.lock().unwrap();
            let idx = errors.len() as u32;
            errors.push(ErrorEntry {
                name: String::new(),
                message: String::new(),
                value: thrown_value,
            });
            value::encode_handle(value::TAG_EXCEPTION, idx)
        },
    );

    let exception_value_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, exception_handle: i64| -> i64 {
            let idx = value::decode_handle(exception_handle) as usize;
            let errors = caller.data().error_table.lock().unwrap();
            errors.get(idx).map(|e| e.value).unwrap_or(value::encode_undefined())
        },
    );

    let date_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, _this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let args: Vec<i64> = if args_count > 0 {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_undefined();
                };
                let data = memory.data(&caller);
                let base = args_base as usize;
                let mut result = Vec::with_capacity(args_count as usize);
                for i in 0..args_count as usize {
                    let offset = base + i * 8;
                    if offset + 8 <= data.len() {
                        let mut bytes = [0u8; 8];
                        bytes.copy_from_slice(&data[offset..offset + 8]);
                        result.push(i64::from_le_bytes(bytes));
                    } else {
                        result.push(value::encode_undefined());
                    }
                }
                result
            } else {
                vec![]
            };

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
                    if s.is_empty() {
                        f64::NAN
                    } else {
                        match DateTime::parse_from_rfc3339(&s) {
                            Ok(dt) => dt.timestamp_millis() as f64,
                            Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S") {
                                Ok(ndt) => ndt.and_utc().timestamp_millis() as f64,
                                Err(_) => match chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
                                    Ok(nd) => nd.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis() as f64,
                                    Err(_) => f64::NAN,
                                },
                            },
                        }
                    }
                } else {
                    f64::NAN
                }
            } else {
                date_args_to_ms(&args, false)
            };

            let state = caller.data();
            let (get_date_fn, get_day_fn, get_full_year_fn, get_hours_fn, get_milliseconds_fn,
                 get_minutes_fn, get_month_fn, get_seconds_fn, get_time_fn, get_timezone_offset_fn,
                 get_utc_date_fn, get_utc_day_fn, get_utc_full_year_fn, get_utc_hours_fn,
                 get_utc_milliseconds_fn, get_utc_minutes_fn, get_utc_month_fn, get_utc_seconds_fn,
                 set_date_fn, set_full_year_fn, set_hours_fn, set_milliseconds_fn,
                 set_minutes_fn, set_month_fn, set_seconds_fn, set_time_fn,
                 set_utc_date_fn, set_utc_full_year_fn, set_utc_hours_fn, set_utc_milliseconds_fn,
                 set_utc_minutes_fn, set_utc_month_fn, set_utc_seconds_fn,
                 to_string_fn, to_date_string_fn, to_time_string_fn, to_iso_string_fn,
                 to_utc_string_fn, to_json_fn, value_of_fn) = {
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
                    create_date_method(state, DateMethodKind::ToISOString),
                    create_date_method(state, DateMethodKind::ToUTCString),
                    create_date_method(state, DateMethodKind::ToJSON),
                    create_date_method(state, DateMethodKind::ValueOf),
                )
            };

            let obj = alloc_host_object_from_caller(&mut caller, 40);
            let ms_val = value::encode_f64(ms);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__date_ms__", ms_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getDate", get_date_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getDay", get_day_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getFullYear", get_full_year_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getHours", get_hours_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getMilliseconds", get_milliseconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getMinutes", get_minutes_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getMonth", get_month_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getSeconds", get_seconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getTime", get_time_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getTimezoneOffset", get_timezone_offset_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCDate", get_utc_date_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCDay", get_utc_day_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCFullYear", get_utc_full_year_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCHours", get_utc_hours_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCMilliseconds", get_utc_milliseconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCMinutes", get_utc_minutes_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCMonth", get_utc_month_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCSeconds", get_utc_seconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setDate", set_date_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setFullYear", set_full_year_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setHours", set_hours_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setMilliseconds", set_milliseconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setMinutes", set_minutes_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setMonth", set_month_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setSeconds", set_seconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setTime", set_time_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCDate", set_utc_date_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCFullYear", set_utc_full_year_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCHours", set_utc_hours_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCMilliseconds", set_utc_milliseconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCMinutes", set_utc_minutes_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCMonth", set_utc_month_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCSeconds", set_utc_seconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toString", to_string_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toDateString", to_date_string_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toTimeString", to_time_string_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toISOString", to_iso_string_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toUTCString", to_utc_string_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toJSON", to_json_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "valueOf", value_of_fn);
            obj
        },
    );

    let date_now_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>| -> i64 {
            let now = chrono::Utc::now();
            value::encode_f64(now.timestamp_millis() as f64)
        },
    );

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
            match DateTime::parse_from_rfc3339(&s) {
                Ok(dt) => value::encode_f64(dt.timestamp_millis() as f64),
                Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S") {
                    Ok(ndt) => value::encode_f64(ndt.and_utc().timestamp_millis() as f64),
                    Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S%.f") {
                        Ok(ndt) => value::encode_f64(ndt.and_utc().timestamp_millis() as f64),
                        Err(_) => match chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
                            Ok(nd) => value::encode_f64(nd.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis() as f64),
                            Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%b %d, %Y") {
                                Ok(ndt) => value::encode_f64(ndt.and_utc().timestamp_millis() as f64),
                                Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%B %d, %Y") {
                                    Ok(ndt) => value::encode_f64(ndt.and_utc().timestamp_millis() as f64),
                                    Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%d %b %Y %H:%M:%S") {
                                        Ok(ndt) => value::encode_f64(ndt.and_utc().timestamp_millis() as f64),
                                        Err(_) => value::encode_f64(f64::NAN),
                                    },
                                },
                            },
                        },
                    },
                },
            }
        },
    );

    let date_utc_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let args = vec![arg];
            let ms = date_args_to_ms(&args, true);
            value::encode_f64(ms)
        },
    );

    // TODO: 当前私有字段实现仅通过 "#fieldName" 字符串作为属性键存储在对象的普通属性槽中，
    // 不符合 ECMAScript 规范的 [[PrivateElements]] 语义。任何代码都可以通过 obj["#x"] 访问，
    // 且没有基于类身份的访问控制。未来需要重构为基于类身份的私有槽机制。
    let private_get_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) {
                *caller.data().runtime_error.lock().expect("runtime error mutex") =
                    Some("TypeError: cannot read private member from non-object".to_string());
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            match read_object_property_by_name_id(&mut caller, ptr, key_name_id as u32) {
                Some(val) => val,
                None => {
                    *caller.data().runtime_error.lock().expect("runtime error mutex") =
                        Some("TypeError: cannot read private member from an object whose class did not declare it".to_string());
                    value::encode_undefined()
                }
            }
        },
    );

    let private_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32, val: i64| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) {
                *caller.data().runtime_error.lock().expect("runtime error mutex") =
                    Some("TypeError: cannot write private member to non-object".to_string());
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let found_slot = find_property_slot_by_name_id(&mut caller, ptr, key_name_id as u32);
            if let Some((slot_offset, _flags, _old_val)) = found_slot {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_undefined();
                };
                let data = memory.data_mut(&mut caller);
                data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
                val
            } else {
                write_object_property_by_name_id(&mut caller, ptr, obj, key_name_id as u32, val, 0);
                val
            }
        },
    );

    let private_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) {
                return value::encode_bool(false);
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_bool(false);
            };
            let found = find_property_slot_by_name_id(&mut caller, ptr, key_name_id as u32);
            value::encode_bool(found.is_some())
        },
    );

    vec![
        map_constructor_fn.into(),              // 251
        map_proto_set_fn.into(),                // 252
        map_proto_get_fn.into(),                // 253
        set_constructor_fn.into(),              // 254
        set_proto_add_fn.into(),                // 255
        map_set_has_fn.into(),                  // 256
        map_set_delete_fn.into(),               // 257
        map_set_clear_fn.into(),                // 258
        map_set_get_size_fn.into(),             // 259
        map_set_for_each_fn.into(),             // 260
        map_set_keys_fn.into(),                 // 261
        map_set_values_fn.into(),               // 262
        map_set_entries_fn.into(),              // 263
        date_constructor_fn.into(),             // 264
        date_now_fn.into(),                     // 265
        date_parse_fn.into(),                   // 266
        date_utc_fn.into(),                     // 267
        // ── WeakMap/WeakSet imports ──
        weakmap_constructor_fn.into(),           // 268
        weakmap_proto_set_fn.into(),             // 269
        weakmap_proto_get_fn.into(),             // 270
        weakmap_proto_has_fn.into(),             // 271
        weakmap_proto_delete_fn.into(),          // 272
        weakset_constructor_fn.into(),           // 273
        weakset_proto_add_fn.into(),             // 274
        weakset_proto_has_fn.into(),             // 275
        weakset_proto_delete_fn.into(),          // 276
        // ── ArrayBuffer imports ──
        arraybuffer_constructor_fn.into(),       // 277
        arraybuffer_proto_byte_length_fn.into(), // 278
        arraybuffer_proto_slice_fn.into(),       // 279
        // ── DataView imports ──
        dataview_constructor_fn.into(),          // 280
        dataview_proto_get_float64_fn.into(),    // 281
        dataview_proto_get_float32_fn.into(),    // 282
        dataview_proto_get_int32_fn.into(),      // 283
        dataview_proto_get_uint32_fn.into(),     // 284
        dataview_proto_get_int16_fn.into(),      // 285
        dataview_proto_get_uint16_fn.into(),     // 286
        dataview_proto_get_int8_fn.into(),       // 287
        dataview_proto_get_uint8_fn.into(),      // 288
        dataview_proto_set_float64_fn.into(),    // 289
        dataview_proto_set_float32_fn.into(),    // 290
        dataview_proto_set_int32_fn.into(),      // 291
        dataview_proto_set_uint32_fn.into(),     // 292
        dataview_proto_set_int16_fn.into(),      // 293
        dataview_proto_set_uint16_fn.into(),     // 294
        dataview_proto_set_int8_fn.into(),       // 295
        dataview_proto_set_uint8_fn.into(),      // 296
        // ── TypedArray constructor imports ──
        int8array_constructor_fn.into(),         // 297
        uint8array_constructor_fn.into(),        // 298
        uint8clampedarray_constructor_fn.into(), // 299
        int16array_constructor_fn.into(),        // 300
        uint16array_constructor_fn.into(),       // 301
        int32array_constructor_fn.into(),        // 302
        uint32array_constructor_fn.into(),       // 303
        float32array_constructor_fn.into(),      // 304
        float64array_constructor_fn.into(),      // 305
        // ── TypedArray prototype imports ──
        typedarray_proto_length_fn.into(),       // 306
        typedarray_proto_byte_length_fn.into(),  // 307
        typedarray_proto_byte_offset_fn.into(),  // 308
        typedarray_proto_set_fn.into(),          // 309
        typedarray_proto_slice_fn.into(),        // 310
        typedarray_proto_subarray_fn.into(),     // 311
        create_global_object_fn.into(),           // 312
        create_exception_fn.into(),               // 313
        exception_value_fn.into(),                // 314
        private_get_fn.into(),                    // 316
        private_set_fn.into(),                    // 317
        private_has_fn.into(),                    // 318
    ]
}
