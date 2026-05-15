use wasmtime::*;
use wjsm_ir::value;
use wjsm_ir::constants;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let define_property_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key: i32, desc: i64| {
            // 检查 obj 和 desc 是否是对象或函数
            if (!value::is_object(obj) && !value::is_function(obj) && !value::is_array(obj))
                || (!value::is_object(desc) && !value::is_function(desc) && !value::is_array(desc))
            {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: Object.defineProperty called on non-object".to_string());
                return;
            }
            let obj_ptr = match resolve_handle(&mut caller, obj) {
                Some(p) => p,
                None => return,
            };
            let desc_ptr = match resolve_handle(&mut caller, desc) {
                Some(p) => p,
                None => return,
            };
            let name_id = key as u32;
            // 读取描述符属性
            let prop_value = read_object_property_by_name(&mut caller, desc_ptr, "value");
            let prop_writable = read_object_property_by_name(&mut caller, desc_ptr, "writable");
            let prop_enumerable = read_object_property_by_name(&mut caller, desc_ptr, "enumerable");
            let prop_configurable =
                read_object_property_by_name(&mut caller, desc_ptr, "configurable");
            let prop_get = read_object_property_by_name(&mut caller, desc_ptr, "get");
            let prop_set = read_object_property_by_name(&mut caller, desc_ptr, "set");

            // 检查是否为访问器属性（有 get 或 set）
            if let Some(getter) = prop_get {
                if !value::is_undefined(getter) && !value::is_callable(getter) {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime error mutex") =
                        Some("TypeError: property getter must be callable".to_string());
                    return;
                }
            }
            if let Some(setter) = prop_set {
                if !value::is_undefined(setter) && !value::is_callable(setter) {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime error mutex") =
                        Some("TypeError: property setter must be callable".to_string());
                    return;
                }
            }

            let is_accessor = prop_get.is_some() || prop_set.is_some();

            // 检查 descriptor 冲突：accessor 和 data 字段不能共存
            // ToPropertyDescriptor: 如果同时有 get/set 和 value/writable，应抛 TypeError
            if is_accessor {
                // 访问器属性不能有 value 或 writable 字段
                if prop_value.is_some() {
                    *caller.data().runtime_error.lock().expect("runtime error mutex") =
                        Some("TypeError: Invalid property descriptor: cannot specify both accessor and value".to_string());
                    return;
                }
                if prop_writable.is_some() {
                    *caller.data().runtime_error.lock().expect("runtime error mutex") =
                        Some("TypeError: Invalid property descriptor: cannot specify both accessor and writable".to_string());
                    return;
                }
            }

            // 计算 flags: bit0=configurable, bit1=enumerable, bit2=writable, bit3=is_accessor
            // JS 规范：缺省的属性特性默认为 false
            let mut flags: i32 = 0;
            if is_accessor {
                flags |= constants::FLAG_IS_ACCESSOR; // is_accessor
            }
            if !is_accessor && prop_writable.map_or(false, |v| !value::is_falsy(v)) {
                flags |= constants::FLAG_WRITABLE; // writable (仅数据属性)
            }
            if prop_enumerable.map_or(false, |v| !value::is_falsy(v)) {
                flags |= constants::FLAG_ENUMERABLE; // enumerable
            }
            if prop_configurable.map_or(false, |v| !value::is_falsy(v)) {
                flags |= constants::FLAG_CONFIGURABLE; // configurable
            }

            let val = prop_value.unwrap_or(value::encode_undefined());
            let getter = prop_get.unwrap_or(value::encode_undefined());
            let setter = prop_set.unwrap_or(value::encode_undefined());

            // 查找已有属性
            let found = find_property_slot_by_name_id(&mut caller, obj_ptr, name_id);
            if let Some((slot_offset, _old_flags, _old_val)) = found {
                // 更新已有属性
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return;
                };
                let data = memory.data_mut(&mut caller);
                data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
                data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
                data[slot_offset + 16..slot_offset + 24].copy_from_slice(&getter.to_le_bytes());
                data[slot_offset + 24..slot_offset + 32].copy_from_slice(&setter.to_le_bytes());
            } else {
                // 添加新属性
                let (capacity, num_props) = {
                    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                        return;
                    };
                    let data = memory.data(&caller);
                    if obj_ptr + 16 > data.len() {
                        return;
                    }
                    let capacity = u32::from_le_bytes([
                        data[obj_ptr + 8],
                        data[obj_ptr + 9],
                        data[obj_ptr + 10],
                        data[obj_ptr + 11],
                    ]) as usize;
                    let num_props = u32::from_le_bytes([
                        data[obj_ptr + 12],
                        data[obj_ptr + 13],
                        data[obj_ptr + 14],
                        data[obj_ptr + 15],
                    ]) as usize;
                    (capacity, num_props)
                };

                // 实际写入用的对象指针（可能因扩容而改变）
                let mut actual_obj_ptr = obj_ptr;

                // 如果容量不足，执行 host 侧扩容
                if num_props >= capacity {
                    // 读取全局变量
                    let obj_table_ptr = {
                        let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else {
                            return;
                        };
                        g.get(&mut caller).i32().unwrap_or(0) as usize
                    };
                    let heap_ptr = {
                        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
                            return;
                        };
                        g.get(&mut caller).i32().unwrap_or(0) as usize
                    };
                    let handle_idx = (obj as u64 & 0xFFFF_FFFF) as u32;

                    // 计算新容量和新大小
                    let new_capacity = if capacity == 0 { 1 } else { capacity * 2 };
                    let new_size = 16 + new_capacity * 32;

                    // 复制旧数据到新位置并更新元数据
                    {
                        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                            return;
                        };
                        let data = memory.data_mut(&mut caller);
                        if heap_ptr + new_size > data.len() {
                            return;
                        }

                        // 复制旧数据（header + 已有属性）
                        let old_size = 16 + num_props * 32;
                        data.copy_within(actual_obj_ptr..actual_obj_ptr + old_size, heap_ptr);

                        // 更新新对象的 capacity
                        data[heap_ptr + 8..heap_ptr + 12]
                            .copy_from_slice(&(new_capacity as u32).to_le_bytes());

                        // 更新 handle 表
                        let slot_addr = obj_table_ptr + handle_idx as usize * 4;
                        if slot_addr + 4 <= data.len() {
                            data[slot_addr..slot_addr + 4]
                                .copy_from_slice(&(heap_ptr as u32).to_le_bytes());
                        }
                    }

                    // 更新 __heap_ptr 全局变量
                    {
                        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
                            return;
                        };
                        let _ = g.set(&mut caller, Val::I32((heap_ptr + new_size) as i32));
                    }

                    actual_obj_ptr = heap_ptr;
                }

                // 写入新属性
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return;
                };
                let data = memory.data_mut(&mut caller);
                let slot_offset = actual_obj_ptr + 16 + num_props * 32;
                if slot_offset + 32 > data.len() {
                    return;
                }
                data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
                data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
                data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
                data[slot_offset + 16..slot_offset + 24].copy_from_slice(&getter.to_le_bytes());
                data[slot_offset + 24..slot_offset + 32].copy_from_slice(&setter.to_le_bytes());
                let new_num_props = num_props + 1;
                data[actual_obj_ptr + 12..actual_obj_ptr + 16]
                    .copy_from_slice(&(new_num_props as u32).to_le_bytes());
            }
        },
    );

    let get_own_prop_desc_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key: i32| -> i64 {
            // 检查 obj 是否是对象或函数
            if !value::is_object(obj) && !value::is_function(obj) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") = Some(
                    "TypeError: Object.getOwnPropertyDescriptor called on non-object".to_string(),
                );
                return value::encode_undefined();
            }

            let obj_ptr = match resolve_handle(&mut caller, obj) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            let name_id = key as u32;

            // 查找属性（仅自身属性）
            let found = find_property_slot_by_name_id(&mut caller, obj_ptr, name_id);
            let Some((slot_offset, flags, _val)) = found else {
                return value::encode_undefined(); // 属性不存在
            };

            // 读取属性槽中的所有值
            let (value, getter, setter) = {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_undefined();
                };
                let data = memory.data(&caller);
                if slot_offset + 32 > data.len() {
                    return value::encode_undefined();
                }
                let value = i64::from_le_bytes([
                    data[slot_offset + 8],
                    data[slot_offset + 9],
                    data[slot_offset + 10],
                    data[slot_offset + 11],
                    data[slot_offset + 12],
                    data[slot_offset + 13],
                    data[slot_offset + 14],
                    data[slot_offset + 15],
                ]);
                let getter = i64::from_le_bytes([
                    data[slot_offset + 16],
                    data[slot_offset + 17],
                    data[slot_offset + 18],
                    data[slot_offset + 19],
                    data[slot_offset + 20],
                    data[slot_offset + 21],
                    data[slot_offset + 22],
                    data[slot_offset + 23],
                ]);
                let setter = i64::from_le_bytes([
                    data[slot_offset + 24],
                    data[slot_offset + 25],
                    data[slot_offset + 26],
                    data[slot_offset + 27],
                    data[slot_offset + 28],
                    data[slot_offset + 29],
                    data[slot_offset + 30],
                    data[slot_offset + 31],
                ]);
                (value, getter, setter)
            };

            // 解析 flags
            let is_accessor = (flags & (1 << 3)) != 0;
            let configurable = (flags & 1) != 0;
            let enumerable = (flags & (1 << 1)) != 0;
            let writable = (flags & (1 << 2)) != 0;

            // 分配描述符对象（需要 4 个属性）
            let desc_handle = match allocate_descriptor_object(
                &mut caller,
                is_accessor,
                value,
                writable,
                enumerable,
                configurable,
                getter,
                setter,
            ) {
                Some(h) => h,
                None => return value::encode_undefined(),
            };

            desc_handle
        },
    );

    // ── Import 19: abstract_eq(i64, i64) → i64 ──────────────────────────────

    let object_rest_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, excluded_keys: i64| -> i64 {
            object_rest_impl(&mut caller, obj, excluded_keys)
        },
    );

    // ── obj_spread (#82): Copy own enumerable properties ────────────────────

    let obj_spread_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, dest: i64, source: i64| {
            obj_spread_impl(&mut caller, dest, source);
        },
    );

    // ── Import 83: has_own_property(i64, i32) -> i64 ──────────────────────────

    let has_own_property_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_ptr: i32| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) && !value::is_array(obj) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: hasOwnProperty called on non-object".to_string());
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_bool(false);
            };
            let found = find_property_slot_by_name_id(&mut caller, ptr, key_ptr as u32);
            value::encode_bool(found.is_some())
        },
    );
    // ── Import 84: obj_keys(i64) -> i64 ───────────────────────────────────────

    let obj_keys_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let names = collect_own_property_names(&mut caller, ptr, true);
            let arr = alloc_array(&mut caller, names.len() as u32);
            let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            for (i, name) in names.iter().enumerate() {
                let key_val = store_runtime_string(&caller, name.clone());
                write_array_elem(&mut caller, arr_ptr, i as u32, key_val);
            }
            write_array_length(&mut caller, arr_ptr, names.len() as u32);
            arr
        },
    );
    // ── Import 85: obj_values(i64) -> i64 ─────────────────────────────────────

    let obj_values_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let values = collect_own_property_values(&mut caller, ptr, true);
            let arr = alloc_array(&mut caller, values.len() as u32);
            let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            for (i, val) in values.iter().enumerate() {
                write_array_elem(&mut caller, arr_ptr, i as u32, *val);
            }
            write_array_length(&mut caller, arr_ptr, values.len() as u32);
            arr
        },
    );
    // ── Import 86: obj_entries(i64) -> i64 ────────────────────────────────────

    let obj_entries_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let names = collect_own_property_names(&mut caller, ptr, true);
            let values = collect_own_property_values(&mut caller, ptr, true);
            let len = names.len().min(values.len());
            let arr = alloc_array(&mut caller, len as u32);
            let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            for i in 0..len {
                // 每个元素是一个 [key, value] 子数组
                let sub_arr = alloc_array(&mut caller, 2);
                let Some(sub_ptr) = resolve_array_ptr(&mut caller, sub_arr) else {
                    continue;
                };
                let key_val = store_runtime_string(&caller, names[i].clone());
                write_array_elem(&mut caller, sub_ptr, 0, key_val);
                write_array_elem(&mut caller, sub_ptr, 1, values[i]);
                write_array_length(&mut caller, sub_ptr, 2);
                write_array_elem(&mut caller, arr_ptr, i as u32, sub_arr);
            }
            write_array_length(&mut caller, arr_ptr, len as u32);
            arr
        },
    );
    // ── Import 87: obj_assign(i64, i64, i32, i32) -> i64 ──────────────────────

    let obj_assign_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         target: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            if !value::is_object(target) && !value::is_function(target) && !value::is_array(target)
            {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: target is not an object".to_string());
                return value::encode_undefined();
            }
            let mut target_ptr = match resolve_handle(&mut caller, target) {
                Some(p) => p,
                None => return target,
            };
            for i in 0..args_count {
                let source_val = read_shadow_arg(&mut caller, args_base, i as u32);
                if !value::is_object(source_val)
                    && !value::is_function(source_val)
                    && !value::is_array(source_val)
                {
                    continue;
                }
                let Some(source_ptr) = resolve_handle(&mut caller, source_val) else {
                    continue;
                };
                // 收集源对象的可枚举属性
                let source_props: Vec<(u32, i32, i64)> = {
                    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                        continue;
                    };
                    let d = mem.data(&caller);
                    if source_ptr + 16 > d.len() {
                        continue;
                    }
                    let num_props = u32::from_le_bytes([
                        d[source_ptr + 12],
                        d[source_ptr + 13],
                        d[source_ptr + 14],
                        d[source_ptr + 15],
                    ]) as usize;
                    let mut props = Vec::new();
                    for j in 0..num_props {
                        let slot_offset = source_ptr + 16 + j * 32;
                        if slot_offset + 32 > d.len() {
                            break;
                        }
                        let flags = i32::from_le_bytes([
                            d[slot_offset + 4],
                            d[slot_offset + 5],
                            d[slot_offset + 6],
                            d[slot_offset + 7],
                        ]);
                        if (flags & 2) == 0 {
                            continue;
                        }
                        let nid = u32::from_le_bytes([
                            d[slot_offset],
                            d[slot_offset + 1],
                            d[slot_offset + 2],
                            d[slot_offset + 3],
                        ]);
                        let vl = i64::from_le_bytes([
                            d[slot_offset + 8],
                            d[slot_offset + 9],
                            d[slot_offset + 10],
                            d[slot_offset + 11],
                            d[slot_offset + 12],
                            d[slot_offset + 13],
                            d[slot_offset + 14],
                            d[slot_offset + 15],
                        ]);
                        props.push((nid, flags, vl));
                    }
                    props
                };
                // 写入目标对象 — 先检查容量再写入，避免静默丢弃属性
                // 1) 统计需新增的属性数（源有而目标无）
                let mut new_count: usize = 0;
                for (name_id, _, _) in &source_props {
                    if find_property_slot_by_name_id(&mut caller, target_ptr, *name_id).is_none() {
                        new_count += 1;
                    }
                }
                // 2) 容量不足则扩容（capacity × 2 倍增）
                if new_count > 0 {
                    let need_grow = {
                        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                            continue;
                        };
                        let d = mem.data(&caller);
                        let num = u32::from_le_bytes([
                            d[target_ptr + 12],
                            d[target_ptr + 13],
                            d[target_ptr + 14],
                            d[target_ptr + 15],
                        ]) as usize;
                        let cap = u32::from_le_bytes([
                            d[target_ptr + 8],
                            d[target_ptr + 9],
                            d[target_ptr + 10],
                            d[target_ptr + 11],
                        ]) as usize;
                        num + new_count > cap
                    };
                    if need_grow {
                        let (num, cap) = {
                            let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                                continue;
                            };
                            let d = mem.data(&caller);
                            let n = u32::from_le_bytes([
                                d[target_ptr + 12],
                                d[target_ptr + 13],
                                d[target_ptr + 14],
                                d[target_ptr + 15],
                            ]) as usize;
                            let c = u32::from_le_bytes([
                                d[target_ptr + 8],
                                d[target_ptr + 9],
                                d[target_ptr + 10],
                                d[target_ptr + 11],
                            ]) as usize;
                            (n, c)
                        };
                        let new_cap = (cap * 2).max(num + new_count) as u32;
                        if let Some(new_ptr) = grow_object(&mut caller, target_ptr, target, new_cap)
                        {
                            target_ptr = new_ptr;
                        }
                    }
                }
                // 3) 写入属性（存在则覆盖值，不存在则追加）
                for (name_id, flags, val) in &source_props {
                    let existing = find_property_slot_by_name_id(&mut caller, target_ptr, *name_id);
                    if let Some((existing_offset, _, _)) = existing {
                        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                            continue;
                        };
                        let d = mem.data_mut(&mut caller);
                        d[existing_offset + 8..existing_offset + 16]
                            .copy_from_slice(&val.to_le_bytes());
                    } else {
                        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                            continue;
                        };
                        let d = mem.data_mut(&mut caller);
                        let target_num_props = u32::from_le_bytes([
                            d[target_ptr + 12],
                            d[target_ptr + 13],
                            d[target_ptr + 14],
                            d[target_ptr + 15],
                        ]) as usize;
                        let new_slot_offset = target_ptr + 16 + target_num_props * 32;
                        d[new_slot_offset..new_slot_offset + 4]
                            .copy_from_slice(&name_id.to_le_bytes());
                        d[new_slot_offset + 4..new_slot_offset + 8]
                            .copy_from_slice(&flags.to_le_bytes());
                        d[new_slot_offset + 8..new_slot_offset + 16]
                            .copy_from_slice(&val.to_le_bytes());
                        let zero: u64 = 0;
                        d[new_slot_offset + 16..new_slot_offset + 24]
                            .copy_from_slice(&zero.to_le_bytes());
                        d[new_slot_offset + 24..new_slot_offset + 32]
                            .copy_from_slice(&zero.to_le_bytes());
                        d[target_ptr + 12..target_ptr + 16]
                            .copy_from_slice(&((target_num_props + 1) as u32).to_le_bytes());
                    }
                }
            }
            target
        },
    );
    // ── Import 88: obj_create(i64, i64) -> i64 ────────────────────────────────

    let obj_create_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, proto: i64, _properties: i64| -> i64 {
            let obj_handle = alloc_object(&mut caller, 0);
            if !value::is_null(proto) && !value::is_undefined(proto) {
                // 设置 __proto__：通过内存写 proto 槽位
                let Some(ptr) = resolve_handle(&mut caller, obj_handle) else {
                    return obj_handle;
                };
                let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                    return obj_handle;
                };
                let d = mem.data_mut(&mut caller);
                if value::is_object(proto) || value::is_function(proto) || value::is_array(proto) {
                    let proto_handle = (proto as u64 & 0xFFFF_FFFF) as u32;
                    d[ptr..ptr + 4].copy_from_slice(&proto_handle.to_le_bytes());
                }
            }
            obj_handle
        },
    );
    // ── Import 89: obj_get_proto_of(i64) -> i64 ───────────────────────────────

    let obj_get_proto_of_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                return value::encode_undefined();
            };
            let d = mem.data(&caller);
            if ptr + 4 > d.len() {
                return value::encode_undefined();
            }
            let proto_handle = u32::from_le_bytes([d[ptr], d[ptr + 1], d[ptr + 2], d[ptr + 3]]);
            if proto_handle == 0xFFFF_FFFF || proto_handle == 0 {
                return value::encode_null();
            }
            let Some(_proto_ptr) = resolve_handle_idx(&mut caller, proto_handle as usize) else {
                return value::encode_null();
            };
            value::encode_object_handle(proto_handle)
        },
    );
    // ── Import 90: obj_set_proto_of(i64, i64) -> i64 ──────────────────────────

    let obj_set_proto_of_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, proto: i64| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) && !value::is_array(obj) {
                return obj; // primitive → no-op per spec
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return obj;
            };
            if value::is_null(proto) || value::is_undefined(proto) {
                // 设置为 null
                let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                    return obj;
                };
                let d = mem.data_mut(&mut caller);
                let null_handle: u32 = 0xFFFF_FFFF;
                d[ptr..ptr + 4].copy_from_slice(&null_handle.to_le_bytes());
                return obj;
            }
            if value::is_object(proto) || value::is_function(proto) || value::is_array(proto) {
                // 循环检测：遍历 proto 的原型链，若 obj 出现在其中则抛出 TypeError
                {
                    let mut current_handle = (proto as u64 & 0xFFFF_FFFF) as u32;
                    let mut depth = 0;
                    const MAX_PROTO_DEPTH: u32 = 1000;
                    let obj_handle = (obj as u64 & 0xFFFF_FFFF) as u32;
                    while current_handle != 0xFFFF_FFFF
                        && current_handle != 0
                        && depth < MAX_PROTO_DEPTH
                    {
                        if current_handle == obj_handle {
                            *caller
                                .data()
                                .runtime_error
                                .lock()
                                .expect("runtime error mutex") =
                                Some("TypeError: Cyclic __proto__ value".to_string());
                            return obj;
                        }
                        let Some(current_ptr) =
                            resolve_handle_idx(&mut caller, current_handle as usize)
                        else {
                            break;
                        };
                        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                            break;
                        };
                        let d = mem.data(&caller);
                        if current_ptr + 4 > d.len() {
                            break;
                        }
                        current_handle = u32::from_le_bytes([
                            d[current_ptr],
                            d[current_ptr + 1],
                            d[current_ptr + 2],
                            d[current_ptr + 3],
                        ]);
                        depth += 1;
                    }
                }
                let proto_handle = (proto as u64 & 0xFFFF_FFFF) as u32;
                let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                    return obj;
                };
                let d = mem.data_mut(&mut caller);
                d[ptr..ptr + 4].copy_from_slice(&proto_handle.to_le_bytes());
            }
            obj
        },
    );
    // ── Import 91: obj_get_own_prop_names(i64) -> i64 ─────────────────────────

    let obj_get_own_prop_names_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let names = collect_own_property_names(&mut caller, ptr, false);
            let arr = alloc_array(&mut caller, names.len() as u32);
            let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            for (i, name) in names.iter().enumerate() {
                let key_val = store_runtime_string(&caller, name.clone());
                write_array_elem(&mut caller, arr_ptr, i as u32, key_val);
            }
            write_array_length(&mut caller, arr_ptr, names.len() as u32);
            arr
        },
    );
    // ── Import 92: obj_is(i64, i64) -> i64 ────────────────────────────────────

    let obj_is_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, val1: i64, val2: i64| -> i64 {
            // SameValue (ECMAScript 7.2.11)
            // 注意: wjsm 使用 NaN-boxing 编码，NaN-boxed 值的高位与 IEEE NaN 重叠，
            // 必须先区分数值类型再应用 IEEE 754 语义，否则 Object.is(null, undefined) 会错误返回 true
            let bits1 = val1 as u64;
            let bits2 = val2 as u64;
            let is_f64_1 = value::is_f64(val1);
            let is_f64_2 = value::is_f64(val2);
            if is_f64_1 && is_f64_2 {
                // 两者都是 IEEE 754 数值（含 signaling NaN）
                // +0 != -0
                if bits1 == 0 && bits2 == 0x8000_0000_0000_0000 {
                    return value::encode_bool(false);
                }
                if bits1 == 0x8000_0000_0000_0000 && bits2 == 0 {
                    return value::encode_bool(false);
                }
                // NaN == NaN (signaling NaN 区域)
                let f1 = f64::from_bits(bits1);
                let f2 = f64::from_bits(bits2);
                if f1.is_nan() && f2.is_nan() {
                    return value::encode_bool(true);
                }
                value::encode_bool(bits1 == bits2)
            } else {
                // 至少一个是 NaN-boxed JS 值（或 canonical quiet NaN）
                // NaN-boxed 值用 bitwise 比较：不同 handle/index 表示不同对象
                value::encode_bool(bits1 == bits2)
            }
        },
    );
    // ── Import 93: obj_proto_to_string(i64) -> i64 ────────────────────────────

    let obj_proto_to_string_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            obj_proto_to_string_impl(&mut caller, obj)
        },
    );
    // ── Import 94: obj_proto_value_of(i64) -> i64 ─────────────────────────────

    let obj_proto_value_of_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, obj: i64| -> i64 { obj },
    );

    // ═══════════════════════════════════════════════════════════════════
    // ── BigInt host functions ──────────────────────────────────────────
    // ═══════════════════════════════════════════════════════════════════

    // ── Import 95: bigint_from_literal(i32, i32) → i64 ─────────────────

    let proxy_create_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, handler: i64| -> i64 {
            if !value::is_object(target) && !value::is_function(target) && !value::is_array(target) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Proxy target must be an object".to_string());
                return value::encode_undefined();
            }
            if !value::is_object(handler) && !value::is_function(handler) && !value::is_array(handler) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Proxy handler must be an object".to_string());
                return value::encode_undefined();
            }
            let handle;
            {
                let mut table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                handle = table.len() as u32;
                table.push(ProxyEntry { target, handler, revoked: false });
            }
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__proxy_handle__", handle_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "proxy_target", target);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "proxy_handler", handler);
            obj
        },
    );

    let reflect_get_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64, receiver: i64| -> i64 {
            let obj_ptr = resolve_handle(&mut caller, target);
            if let Some(ptr) = obj_ptr {
                if let Some(prop_name) = render_value(&mut caller, prop).ok() {
                    if let Some(val) = read_object_property_by_name(&mut caller, ptr, &prop_name) {
                        return val;
                    }
                }
            }
            let _ = receiver;
            value::encode_undefined()
        },
    );

    let proxy_revocable_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, handler: i64| -> i64 {
            let proxy_val = {
                let handle;
                {
                    let mut table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                    handle = table.len() as u32;
                    table.push(ProxyEntry { target, handler, revoked: false });
                }
                let obj = alloc_host_object_from_caller(&mut caller, 4);
                let handle_val = value::encode_f64(handle as f64);
                let _ = define_host_data_property_from_caller(&mut caller, obj, "__proxy_handle__", handle_val);
                let _ = define_host_data_property_from_caller(&mut caller, obj, "proxy_target", target);
                let _ = define_host_data_property_from_caller(&mut caller, obj, "proxy_handler", handler);
                obj
            };
            let obj = alloc_host_object_from_caller(&mut caller, 2);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "proxy", proxy_val);
            let revoke_fn = alloc_host_object_from_caller(&mut caller, 0);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "revoke", revoke_fn);
            obj
        },
    );

    let reflect_set_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64, val: i64, _receiver: i64| -> i64 {
            let obj_ptr = resolve_handle(&mut caller, target);
            if let Some(_ptr) = obj_ptr {
                if let Some(prop_name) = render_value(&mut caller, prop).ok() {
                    let _ = define_host_data_property_from_caller(&mut caller, target, &prop_name, val);
                    return value::encode_bool(true);
                }
            }
            value::encode_bool(false)
        },
    );

    let reflect_has_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64| -> i64 {
            let obj_ptr = resolve_handle(&mut caller, target);
            if let Some(ptr) = obj_ptr {
                if let Some(prop_name) = render_value(&mut caller, prop).ok() {
                    if let Some(name_id) = find_memory_c_string(&mut caller, &prop_name) {
                        let found = find_property_slot_by_name_id(&mut caller, ptr, name_id).is_some();
                        return value::encode_bool(found);
                    }
                }
            }
            value::encode_bool(false)
        },
    );

    let reflect_delete_property_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _target: i64, _prop: i64| -> i64 {
            value::encode_bool(true)
        },
    );

    let reflect_apply_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, this_arg: i64, _args: i64| -> i64 {
            resolve_and_call(&mut caller, target, this_arg, 0, 0)
        },
    );

    let reflect_construct_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, _target: i64, _args: i64, _new_target: i64| -> i64 {
            alloc_host_object_from_caller(&mut caller, 4)
        },
    );

    let reflect_get_prototype_of_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _target: i64| -> i64 {
            value::encode_null()
        },
    );

    let reflect_set_prototype_of_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _target: i64, _proto: i64| -> i64 {
            value::encode_bool(true)
        },
    );

    let reflect_is_extensible_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _target: i64| -> i64 {
            value::encode_bool(true)
        },
    );

    let reflect_prevent_extensions_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _target: i64| -> i64 {
            value::encode_bool(true)
        },
    );

    let reflect_get_own_property_descriptor_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _target: i64, _prop: i64| -> i64 {
            value::encode_undefined()
        },
    );

    let reflect_define_property_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _target: i64, _prop: i64, _descriptor: i64| -> i64 {
            value::encode_bool(true)
        },
    );

    let reflect_own_keys_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _target: i64| -> i64 {
            value::encode_undefined()
        },
    );
    // ── Math builtins ────────────────────────────────────────────────────────

    vec![
        (18, define_property_fn),
        (19, get_own_prop_desc_fn),
        (81, object_rest_fn),
        (82, obj_spread_fn),
        (83, has_own_property_fn),
        (84, obj_keys_fn),
        (85, obj_values_fn),
        (86, obj_entries_fn),
        (87, obj_assign_fn),
        (88, obj_create_fn),
        (89, obj_get_proto_of_fn),
        (90, obj_set_proto_of_fn),
        (91, obj_get_own_prop_names_fn),
        (92, obj_is_fn),
        (93, obj_proto_to_string_fn),
        (94, obj_proto_value_of_fn),
        (151, proxy_create_fn),
        (152, proxy_revocable_fn),
        (153, reflect_get_fn),
        (154, reflect_set_fn),
        (155, reflect_has_fn),
        (156, reflect_delete_property_fn),
        (157, reflect_apply_fn),
        (158, reflect_construct_fn),
        (159, reflect_get_prototype_of_fn),
        (160, reflect_set_prototype_of_fn),
        (161, reflect_is_extensible_fn),
        (162, reflect_prevent_extensions_fn),
        (163, reflect_get_own_property_descriptor_fn),
        (164, reflect_define_property_fn),
        (165, reflect_own_keys_fn),
    ]
}
