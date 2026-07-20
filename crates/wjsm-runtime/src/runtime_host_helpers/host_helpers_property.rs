use super::*;
use crate::runtime_string::RuntimeString;

pub(crate) fn define_host_data_property_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    obj: i64,
    name: &str,
    val: i64,
) -> Option<()> {
    #[cfg(feature = "managed-heap-v2")]
    {
        let handle = value::decode_handle(obj);
        let access = ctx.as_context().data().heap_access_v2().clone();
        if access.resolve_handle(handle).is_ok() {
            let key = crate::property_key::encode_runtime_string_name_id(
                crate::property_key::intern_runtime_property_key(
                    ctx.as_context().data(),
                    RuntimeString::from_utf8_str(name),
                ),
            );
            return access
                .define_data_property(
                    handle,
                    key,
                    val as u64,
                    (constants::FLAG_CONFIGURABLE
                        | constants::FLAG_ENUMERABLE
                        | constants::FLAG_WRITABLE) as u32,
                )
                .map_err(|error| {
                    set_runtime_error(
                        ctx.as_context().data(),
                        format!("V2 host property {name}: {error}"),
                    );
                })
                .ok();
        }
    }
    let name_id = find_memory_c_string_with_env(ctx, env, name)
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, name))?;
    define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        obj,
        encode_string_name_id(name_id),
        val,
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE,
    )
}

pub(crate) fn define_host_data_property_by_name_id_with_env<
    C: AsContextMut<Data = RuntimeState>,
>(
    ctx: &mut C,
    env: &WasmEnv,
    obj: i64,
    name_id: u32,
    val: i64,
    flags: i32,
) -> Option<()> {
    #[cfg(feature = "managed-heap-v2")]
    {
        let handle = value::decode_handle(obj);
        let access = ctx.as_context().data().heap_access_v2().clone();
        if access.resolve_handle(handle).is_ok() {
            let key = crate::property_key::canonicalize_v2_name_id_with_env(ctx, env, name_id)?;
            return access
                .define_data_property(handle, key, val as u64, flags as u32)
                .map_err(|error| {
                    set_runtime_error(
                        ctx.as_context().data(),
                        format!("V2 host property key {name_id}: {error}"),
                    );
                })
                .ok();
        }
    }
    let obj_ptr = resolve_handle_idx_with_env(ctx, env, value::decode_object_handle(obj) as usize)?;
    let (capacity, num_props) = {
        let data = env.memory.data(&*ctx);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        let capacity = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]);
        let num_props = u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]);
        (capacity, num_props)
    };
    if let Some((slot_offset, _, _)) =
        find_property_slot_by_name_id_with_env(ctx, env, obj_ptr, name_id)
    {
        let slot_idx = (slot_offset - (obj_ptr + 16)) / 32;
        crate::runtime_gc::heap_access::write_property_slot(
            ctx,
            env,
            value::decode_object_handle(obj),
            slot_idx,
            crate::runtime_gc::heap_access::SlotPart::Value,
            val,
        )?;
        return Some(());
    }
    if num_props >= capacity {
        return None;
    }
    let slot_idx = num_props as usize;
    let slot_offset = obj_ptr + 16 + slot_idx * 32;
    {
        let data = env.memory.data_mut(&mut *ctx);
        if slot_offset + 32 > data.len() {
            return None;
        }
        data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
        data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    }
    let undef = value::encode_undefined();
    crate::runtime_gc::heap_access::write_property_slot(
        ctx,
        env,
        value::decode_object_handle(obj),
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Value,
        val,
    )?;
    crate::runtime_gc::heap_access::write_property_slot(
        ctx,
        env,
        value::decode_object_handle(obj),
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Getter,
        undef,
    )?;
    crate::runtime_gc::heap_access::write_property_slot(
        ctx,
        env,
        value::decode_object_handle(obj),
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Setter,
        undef,
    )?;
    let data = env.memory.data_mut(&mut *ctx);
    data[obj_ptr + 12..obj_ptr + 16].copy_from_slice(&(num_props + 1).to_le_bytes());
    Some(())
}

/// 定义一个访问器（getter/setter）属性到宿主创建的对象上（泛型版本，不支持 grow_object）。
/// slot 布局与数据属性相同（32字节），但 flags 标记为 IS_ACCESSOR，
/// offset 8 = undefined（保留），offset 16 = getter，offset 24 = setter。
pub(crate) fn define_host_accessor_property_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    obj: i64,
    name: &str,
    getter: i64,
    setter: i64,
) -> Option<()> {
    #[cfg(feature = "managed-heap-v2")]
    {
        let handle = value::decode_handle(obj);
        let access = ctx.as_context().data().heap_access_v2().clone();
        if access.resolve_handle(handle).is_ok() {
            let key = crate::property_key::encode_runtime_string_name_id(
                crate::property_key::intern_runtime_property_key(
                    ctx.as_context().data(),
                    RuntimeString::from_utf8_str(name),
                ),
            );
            return access
                .define_accessor_property(handle, key, getter as u64, setter as u64)
                .map_err(|error| {
                    set_runtime_error(
                        ctx.as_context().data(),
                        format!("V2 host accessor {name}: {error}"),
                    );
                })
                .ok();
        }
    }
    let name_id = find_memory_c_string_with_env(ctx, env, name)
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, name))?;
    let obj_ptr = resolve_handle_idx_with_env(ctx, env, value::decode_object_handle(obj) as usize)?;
    let (capacity, num_props) = {
        let data = env.memory.data(&*ctx);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        let capacity = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]);
        let num_props = u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]);
        (capacity, num_props)
    };
    if num_props >= capacity {
        return None;
    }
    let flags =
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_IS_ACCESSOR;
    let slot_idx = num_props as usize;
    let slot_offset = obj_ptr + 16 + slot_idx * 32;
    {
        let data = env.memory.data_mut(&mut *ctx);
        if slot_offset + 32 > data.len() {
            return None;
        }
        data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
        data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    }
    let undef = value::encode_undefined();
    crate::runtime_gc::heap_access::write_property_slot(
        ctx,
        env,
        value::decode_object_handle(obj),
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Value,
        undef,
    )?;
    crate::runtime_gc::heap_access::write_property_slot(
        ctx,
        env,
        value::decode_object_handle(obj),
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Getter,
        getter,
    )?;
    crate::runtime_gc::heap_access::write_property_slot(
        ctx,
        env,
        value::decode_object_handle(obj),
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Setter,
        setter,
    )?;
    let data = env.memory.data_mut(&mut *ctx);
    data[obj_ptr + 12..obj_ptr + 16].copy_from_slice(&(num_props + 1).to_le_bytes());
    Some(())
}

pub(crate) fn alloc_promise_all_settled_result(
    caller: &mut Caller<'_, RuntimeState>,
    status: &str,
    value_name: &str,
    value: i64,
) -> i64 {
    alloc_all_settled_result_from_caller(caller, status, value_name, value)
}

pub(crate) fn alloc_aggregate_error(caller: &mut Caller<'_, RuntimeState>, errors: i64) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    alloc_heap_aggregate_error(caller, &env, errors)
}
// ── 辅助函数：检查字符串是否是规范整数索引（ECMAScript §10.1.12 OrdinaryOwnPropertyKeys）──
// 返回 Some(数字值) 如果是规范的整数索引字符串（即 parse 回来再转回字符串保持一致），否则 None。
fn canonical_integer_index(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 10 {
        return None;
    }
    // 不能有前导零，除非是 "0" 本身
    if s.len() > 1 && s.as_bytes()[0] == b'0' {
        return None;
    }
    // 所有字符必须是数字
    if !s.as_bytes().iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // parse 并验证不超过 u32::MAX
    if let Ok(idx) = s.parse::<u64>()
        && idx <= u32::MAX as u64
    {
        // 确保往返转换一致（排除非规范形式如 "+0", "-0" 等，这里 digits-only 已排除）
        let back = idx.to_string();
        if back == s {
            return Some(idx as u32);
        }
    }
    None
}

fn name_id_to_runtime_property_string(
    caller: &mut Caller<'_, RuntimeState>,
    name_id: u32,
) -> Option<RuntimeString> {
    match decode_name_id(name_id) {
        DecodedNameId::MemoryString(index) => Some(RuntimeString::from_utf8_lossy(
            &crate::runtime_render::read_string_bytes(caller, index),
        )),
        DecodedNameId::RuntimeString(index) => runtime_property_key_units(caller.data(), index),
        DecodedNameId::Symbol(_) => None,
    }
}

/// V2 handle 的 own 形状快照；非 V2 handle 返回 None（走 legacy memory32 布局）。
#[cfg(feature = "managed-heap-v2")]
enum V2OwnShape {
    Array { length: u32 },
    Object { slots: Vec<(u32, u32)> },
}

#[cfg(feature = "managed-heap-v2")]
fn v2_own_shape(caller: &Caller<'_, RuntimeState>, obj_ptr: usize) -> Option<V2OwnShape> {
    let handle = u32::try_from(obj_ptr).ok()?;
    let access = caller.data().heap_access_v2();
    access.resolve_handle(handle).ok()?;
    if access.object_type(handle).ok()? == u32::from(wjsm_ir::HEAP_TYPE_ARRAY) {
        Some(V2OwnShape::Array {
            length: access.array_length(handle).ok()?,
        })
    } else {
        Some(V2OwnShape::Object {
            slots: access.own_property_slots(handle).ok()?,
        })
    }
}

/// V2 属性槽 → 过滤 private/enumerable/symbol 后的 name_id 列表。
#[cfg(feature = "managed-heap-v2")]
fn v2_filter_slot_name_ids(
    slots: Vec<(u32, u32)>,
    enumerable_only: bool,
    keep_symbols: bool,
) -> Vec<u32> {
    slots
        .into_iter()
        .filter(|(_, flags)| flags & constants::FLAG_PRIVATE as u32 == 0)
        .filter(|(_, flags)| !enumerable_only || flags & constants::FLAG_ENUMERABLE as u32 != 0)
        .map(|(key, _)| key)
        .filter(|key| keep_symbols || !is_symbol_name_id(*key))
        .collect()
}

/// name_id 列表 → 规范排序的字符串键（整数索引升序在前，其余保持插入顺序）。
#[cfg(feature = "managed-heap-v2")]
fn property_name_strings_from_name_ids(
    caller: &mut Caller<'_, RuntimeState>,
    name_ids: Vec<u32>,
) -> Vec<String> {
    let mut int_index_names = Vec::new();
    let mut string_names = Vec::new();
    for name_id in name_ids {
        let Some(name) = name_id_to_runtime_property_string(caller, name_id) else {
            continue;
        };
        let name = name.to_utf8_lossy();
        if let Some(int_idx) = canonical_integer_index(&name) {
            int_index_names.push((int_idx, name));
        } else {
            string_names.push(name);
        }
    }
    int_index_names.sort_by_key(|(idx, _)| *idx);
    let mut names: Vec<String> = int_index_names.into_iter().map(|(_, name)| name).collect();
    names.extend(string_names);
    names
}
pub(crate) fn collect_own_property_names(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    enumerable_only: bool,
) -> Vec<String> {
    #[cfg(feature = "managed-heap-v2")]
    if let Some(shape) = v2_own_shape(caller, obj_ptr) {
        return match shape {
            V2OwnShape::Array { length } => {
                let mut names: Vec<String> = (0..length).map(|i| i.to_string()).collect();
                if !enumerable_only {
                    names.push("length".to_string());
                }
                names
            }
            V2OwnShape::Object { slots } => {
                let name_ids = v2_filter_slot_name_ids(slots, enumerable_only, false);
                property_name_strings_from_name_ids(caller, name_ids)
            }
        };
    }
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return vec![];
    };
    let data = mem.data(&*caller);
    if obj_ptr + 16 > data.len() {
        return vec![];
    }
    if data[obj_ptr + 4] == wjsm_ir::HEAP_TYPE_ARRAY {
        let len = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]);
        let _ = data;
        let _ = mem;
        let mut names = Vec::new();
        for i in 0..len {
            if array_elem_present(caller, obj_ptr, i) {
                names.push(i.to_string());
            }
        }
        if !enumerable_only {
            names.push("length".to_string());
        }
        return names;
    }
    let num_props = u32::from_le_bytes([
        data[obj_ptr + 12],
        data[obj_ptr + 13],
        data[obj_ptr + 14],
        data[obj_ptr + 15],
    ]) as usize;
    let mut name_ids = Vec::new();
    for i in 0..num_props {
        let slot_offset = obj_ptr + 16 + i * 32;
        if slot_offset + 32 > data.len() {
            break;
        }
        let flags = i32::from_le_bytes([
            data[slot_offset + 4],
            data[slot_offset + 5],
            data[slot_offset + 6],
            data[slot_offset + 7],
        ]);
        if (flags & constants::FLAG_PRIVATE) != 0 {
            continue;
        }
        if enumerable_only && (flags & 2) == 0 {
            continue;
        }
        let name_id = u32::from_le_bytes([
            data[slot_offset],
            data[slot_offset + 1],
            data[slot_offset + 2],
            data[slot_offset + 3],
        ]);
        if is_symbol_name_id(name_id) {
            continue;
        }
        name_ids.push(name_id);
    }
    let _ = data;
    let _ = mem;
    let mut string_ids = Vec::new();
    let mut int_index_names = Vec::new();
    for name_id in name_ids {
        let name_bytes = read_string_bytes(caller, name_id);
        let name = String::from_utf8_lossy(&name_bytes).to_string();
        if let Some(int_idx) = canonical_integer_index(&name) {
            int_index_names.push((int_idx, name));
        } else {
            string_ids.push(name);
        }
    }
    // 按规范排序：整数索引键按数值升序，然后字符串键保持插入顺序
    int_index_names.sort_by_key(|(idx, _)| *idx);
    let mut names: Vec<String> = int_index_names.into_iter().map(|(_, name)| name).collect();
    names.extend(string_ids);
    names
}

pub(crate) fn collect_own_property_string_key_values(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    obj_ptr: usize,
    enumerable_only: bool,
) -> Vec<i64> {
    #[cfg(feature = "managed-heap-v2")]
    if let Some(shape) = v2_own_shape(caller, obj_ptr) {
        return match shape {
            V2OwnShape::Array { length } => {
                let mut keys: Vec<i64> = (0..length)
                    .map(|i| {
                        store_runtime_string(caller, RuntimeString::from_utf8_str(&i.to_string()))
                    })
                    .collect();
                if !enumerable_only {
                    keys.push(store_runtime_string(
                        caller,
                        RuntimeString::from_utf8_str("length"),
                    ));
                }
                let name_ids =
                    crate::array_named_props::ArrayNamedPropsStore::collect_string_name_ids(
                        caller,
                        obj,
                        enumerable_only,
                    );
                for name_id in name_ids {
                    if let Some(name) = name_id_to_runtime_property_string(caller, name_id) {
                        keys.push(store_runtime_string(caller, name));
                    }
                }
                keys
            }
            V2OwnShape::Object { slots } => {
                let name_ids = v2_filter_slot_name_ids(slots, enumerable_only, false);
                property_name_strings_from_name_ids(caller, name_ids)
                    .into_iter()
                    .map(|name| store_runtime_string(caller, name))
                    .collect()
            }
        };
    }
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return vec![];
    };
    let data = mem.data(&*caller);
    if obj_ptr + 16 > data.len() {
        return vec![];
    }
    if data[obj_ptr + 4] == wjsm_ir::HEAP_TYPE_ARRAY {
        let len = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]);
        let _ = data;
        let _ = mem;
        let mut keys = Vec::new();
        for i in 0..len {
            if array_elem_present(caller, obj_ptr, i) {
                keys.push(store_runtime_string(
                    caller,
                    RuntimeString::from_utf8_str(&i.to_string()),
                ));
            }
        }
        if !enumerable_only {
            keys.push(store_runtime_string(
                caller,
                RuntimeString::from_utf8_str("length"),
            ));
        }
        let name_ids = crate::array_named_props::ArrayNamedPropsStore::collect_string_name_ids(
            caller,
            obj,
            enumerable_only,
        );
        for name_id in name_ids {
            if let Some(name) = name_id_to_runtime_property_string(caller, name_id) {
                keys.push(store_runtime_string(caller, name));
            }
        }
        return keys;
    }
    let num_props = u32::from_le_bytes([
        data[obj_ptr + 12],
        data[obj_ptr + 13],
        data[obj_ptr + 14],
        data[obj_ptr + 15],
    ]) as usize;
    let mut name_ids = Vec::new();
    for i in 0..num_props {
        let slot_offset = obj_ptr + 16 + i * 32;
        if slot_offset + 32 > data.len() {
            break;
        }
        let flags = i32::from_le_bytes([
            data[slot_offset + 4],
            data[slot_offset + 5],
            data[slot_offset + 6],
            data[slot_offset + 7],
        ]);
        if (flags & constants::FLAG_PRIVATE) != 0 {
            continue;
        }
        if enumerable_only && (flags & constants::FLAG_ENUMERABLE) == 0 {
            continue;
        }
        let name_id = u32::from_le_bytes([
            data[slot_offset],
            data[slot_offset + 1],
            data[slot_offset + 2],
            data[slot_offset + 3],
        ]);
        if !is_symbol_name_id(name_id) {
            name_ids.push(name_id);
        }
    }
    let _ = data;
    let _ = mem;

    let mut int_index_names = Vec::new();
    let mut string_names = Vec::new();
    for name_id in name_ids {
        let Some(name) = name_id_to_runtime_property_string(caller, name_id) else {
            continue;
        };
        let name_lossy = name.to_utf8_lossy();
        if let Some(int_idx) = canonical_integer_index(&name_lossy) {
            int_index_names.push((int_idx, name));
        } else {
            string_names.push(name);
        }
    }
    int_index_names.sort_by_key(|(idx, _)| *idx);
    let mut keys: Vec<i64> = int_index_names
        .into_iter()
        .map(|(_, name)| store_runtime_string(caller, name))
        .collect();
    for name in string_names {
        keys.push(store_runtime_string(caller, name));
    }
    keys
}

/// 从 boxed 对象/数组收集 own 字符串属性名（数组含侧表命名属性）。
pub(crate) fn collect_own_property_names_from_value(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
    enumerable_only: bool,
) -> Vec<String> {
    let Some(ptr) = resolve_handle(caller, val) else {
        return Vec::new();
    };
    let mut names = collect_own_property_names(caller, ptr, enumerable_only);
    if value::is_array(val) {
        names.extend(
            crate::array_named_props::ArrayNamedPropsStore::collect_string_property_names(
                caller,
                val,
                enumerable_only,
            ),
        );
    }
    names
}

pub(crate) fn collect_own_property_values(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    enumerable_only: bool,
) -> Vec<i64> {
    #[cfg(feature = "managed-heap-v2")]
    if let Some(shape) = v2_own_shape(caller, obj_ptr) {
        let handle = obj_ptr as u32;
        let access = caller.data().heap_access_v2().clone();
        return match shape {
            V2OwnShape::Array { length } => {
                let mut values: Vec<i64> = (0..length)
                    .filter_map(|i| access.get_element(handle, i).ok().flatten())
                    .map(|element| element as i64)
                    .filter(|element| !value::is_array_hole(*element))
                    .collect();
                if !enumerable_only {
                    values.push(value::encode_f64(length as f64));
                }
                values
            }
            V2OwnShape::Object { slots } => v2_filter_slot_name_ids(slots, enumerable_only, false)
                .into_iter()
                .filter_map(|key| access.get_property(handle, key).ok().flatten())
                .map(|property_value| property_value as i64)
                .collect(),
        };
    }
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return vec![];
    };
    let data = mem.data(&*caller);
    if obj_ptr + 16 > data.len() {
        return vec![];
    }
    if data[obj_ptr + 4] == wjsm_ir::HEAP_TYPE_ARRAY {
        let len = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]);
        let _ = data;
        let _ = mem;
        let mut values = Vec::new();
        for i in 0..len {
            if let Some(value) = read_array_elem(caller, obj_ptr, i) {
                values.push(value);
            }
        }
        if !enumerable_only {
            values.push(value::encode_f64(len as f64));
        }
        return values;
    }
    let num_props = u32::from_le_bytes([
        data[obj_ptr + 12],
        data[obj_ptr + 13],
        data[obj_ptr + 14],
        data[obj_ptr + 15],
    ]) as usize;
    let mut values = Vec::new();
    for i in 0..num_props {
        let slot_offset = obj_ptr + 16 + i * 32;
        if slot_offset + 32 > data.len() {
            break;
        }
        let flags = i32::from_le_bytes([
            data[slot_offset + 4],
            data[slot_offset + 5],
            data[slot_offset + 6],
            data[slot_offset + 7],
        ]);
        if (flags & constants::FLAG_PRIVATE) != 0 {
            continue;
        }
        if enumerable_only && (flags & 2) == 0 {
            continue;
        }
        let name_id = u32::from_le_bytes([
            data[slot_offset],
            data[slot_offset + 1],
            data[slot_offset + 2],
            data[slot_offset + 3],
        ]);
        if is_symbol_name_id(name_id) {
            continue;
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
        values.push(value);
    }
    values
}

pub(crate) fn collect_own_property_key_values(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    symbols_only: bool,
) -> Vec<i64> {
    #[cfg(feature = "managed-heap-v2")]
    if let Some(shape) = v2_own_shape(caller, obj_ptr) {
        return match shape {
            V2OwnShape::Array { length } => {
                if symbols_only {
                    return crate::array_named_props::ArrayNamedPropsStore::collect_property_key_values_by_ptr(
                        caller, obj_ptr, true,
                    );
                }
                let mut keys: Vec<i64> = (0..length)
                    .map(|i| store_runtime_string(caller, i.to_string()))
                    .collect();
                keys.push(store_runtime_string(caller, "length".to_string()));
                keys.extend(
                    crate::array_named_props::ArrayNamedPropsStore::collect_property_key_values_by_ptr(
                        caller, obj_ptr, false,
                    ),
                );
                keys
            }
            V2OwnShape::Object { slots } => {
                let name_ids = v2_filter_slot_name_ids(slots, false, true);
                let mut string_keys = Vec::new();
                let mut sym_keys = Vec::new();
                let mut int_index_entries = Vec::new();
                for name_id in name_ids {
                    match decode_name_id(name_id) {
                        DecodedNameId::Symbol(_) => {
                            if let Some(symbol_key) = name_id_to_property_key_value(name_id) {
                                sym_keys.push(symbol_key);
                            }
                        }
                        _ if !symbols_only => {
                            let Some(name) = name_id_to_runtime_property_string(caller, name_id)
                            else {
                                continue;
                            };
                            let name_lossy = name.to_utf8_lossy();
                            if let Some(int_idx) = canonical_integer_index(&name_lossy) {
                                int_index_entries.push((int_idx, name));
                            } else {
                                string_keys.push(name);
                            }
                        }
                        _ => {}
                    }
                }
                int_index_entries.sort_by_key(|(idx, _)| *idx);
                let mut keys: Vec<i64> = int_index_entries
                    .into_iter()
                    .map(|(_, name)| store_runtime_string(caller, name))
                    .collect();
                for name in string_keys {
                    keys.push(store_runtime_string(caller, name));
                }
                keys.extend(sym_keys);
                keys
            }
        };
    }
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return vec![];
    };
    let data = mem.data(&*caller);
    if obj_ptr + 16 > data.len() {
        return vec![];
    }
    if data[obj_ptr + 4] == wjsm_ir::HEAP_TYPE_ARRAY {
        let len = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]);
        let _ = data;
        let _ = mem;
        if symbols_only {
            return crate::array_named_props::ArrayNamedPropsStore::collect_property_key_values_by_ptr(
                caller, obj_ptr, true,
            );
        }
        let mut keys = Vec::new();
        for i in 0..len {
            if array_elem_present(caller, obj_ptr, i) {
                keys.push(store_runtime_string(caller, i.to_string()));
            }
        }
        keys.push(store_runtime_string(caller, "length".to_string()));
        keys.extend(
            crate::array_named_props::ArrayNamedPropsStore::collect_property_key_values_by_ptr(
                caller, obj_ptr, false,
            ),
        );
        return keys;
    }

    let num_props = u32::from_le_bytes([
        data[obj_ptr + 12],
        data[obj_ptr + 13],
        data[obj_ptr + 14],
        data[obj_ptr + 15],
    ]) as usize;
    let mut slots = Vec::new();
    for i in 0..num_props {
        let slot_offset = obj_ptr + 16 + i * 32;
        if slot_offset + 32 > data.len() {
            break;
        }
        let flags = i32::from_le_bytes([
            data[slot_offset + 4],
            data[slot_offset + 5],
            data[slot_offset + 6],
            data[slot_offset + 7],
        ]);
        if (flags & constants::FLAG_PRIVATE) != 0 {
            continue;
        }
        let name_id = u32::from_le_bytes([
            data[slot_offset],
            data[slot_offset + 1],
            data[slot_offset + 2],
            data[slot_offset + 3],
        ]);
        slots.push(name_id);
    }
    let _ = data;
    let _ = mem;

    let mut string_keys = Vec::new();
    let mut sym_name_ids = Vec::new();
    let mut int_index_entries = Vec::new();
    for name_id in slots {
        match decode_name_id(name_id) {
            DecodedNameId::Symbol(_) => {
                if let Some(symbol_key) = name_id_to_property_key_value(name_id) {
                    sym_name_ids.push(symbol_key);
                }
            }
            _ if !symbols_only => {
                let Some(name) = name_id_to_runtime_property_string(caller, name_id) else {
                    continue;
                };
                let name_lossy = name.to_utf8_lossy();
                if let Some(int_idx) = canonical_integer_index(&name_lossy) {
                    int_index_entries.push((int_idx, name));
                } else {
                    string_keys.push(name);
                }
            }
            _ => {}
        }
    }

    // 按规范排序：整数索引键按数值升序，然后字符串键保持插入顺序，最后 Symbol 键保持插入顺序
    int_index_entries.sort_by_key(|(idx, _)| *idx);
    let mut keys: Vec<i64> = int_index_entries
        .into_iter()
        .map(|(_, name)| store_runtime_string(caller, name))
        .collect();
    for name in string_keys {
        keys.push(store_runtime_string(caller, name));
    }
    keys.extend(sym_name_ids);
    keys
}
