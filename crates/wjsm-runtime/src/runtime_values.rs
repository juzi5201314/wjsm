use super::*;
use crate::runtime_string::RuntimeString;
use crate::wasm_env::WasmEnv;

/// 对象属性容量倍增并至少容纳 needed；溢出或超出 u32 容量时返回 None。
fn grown_object_capacity(capacity: usize, needed: usize) -> Option<u32> {
    let doubled = capacity.max(1).checked_mul(2)?;
    let grown = doubled.max(needed);
    u32::try_from(grown).ok()
}
/// 计算 boxed value 在 obj_table 中的 handle 索引。
/// 函数值低 32 位是函数表索引；其属性对象 handle 从 __function_props_base 起算
//（startup snapshot 拆分后 primordial 原型占据更低 handle）。其余值的 handle 即低 32 位。
/// 所有"函数值 → 属性对象 handle"的解析（读/写/扩容）都必须经此函数，避免 read/write 漂移。
pub(crate) fn handle_index_of(caller: &mut Caller<'_, RuntimeState>, val: i64) -> usize {
    let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
    if value::is_function(val) {
        let base = caller
            .get_export("__function_props_base")
            .and_then(Extern::into_global)
            .and_then(|global| global.get(&mut *caller).i32())
            .unwrap_or(0)
            .max(0) as usize;
        return handle_idx.saturating_add(base);
    }
    if value::is_closure(val) {
        let func_idx = caller
            .data()
            .closures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(handle_idx)
            .map(|e| e.func_idx as usize)
            .unwrap_or(0);
        let base = caller
            .get_export("__function_props_base")
            .and_then(Extern::into_global)
            .and_then(|global| global.get(&mut *caller).i32())
            .unwrap_or(0)
            .max(0) as usize;
        return func_idx.saturating_add(base);
    }
    handle_idx
}

/// `handle_index_of` 的 WasmEnv 版本，供 Store/Caller 共用（如 unhandled rejection 渲染）。
pub(crate) fn handle_index_of_with_env<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
    val: i64,
) -> usize {
    let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
    let base = env
        .function_props_base
        .and_then(|g| g.get(&mut *ctx).i32())
        .unwrap_or(0)
        .max(0) as usize;
    if value::is_function(val) {
        return handle_idx.saturating_add(base);
    }
    if value::is_closure(val) {
        let func_idx = ctx
            .state_mut()
            .closures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(handle_idx)
            .map(|e| e.func_idx as usize)
            .unwrap_or(0);
        return func_idx.saturating_add(base);
    }
    handle_idx
}

pub(crate) fn resolve_handle_with_env<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    val: i64,
) -> Option<usize> {
    let handle_idx = handle_index_of_with_env(ctx, env, val);
    resolve_handle_idx_with_env(ctx, env, handle_idx)
}

pub(crate) fn weak_target_handle_index_of(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
) -> Option<u32> {
    if !value::is_js_object(target) {
        return None;
    }
    Some(handle_index_of(caller, target) as u32)
}

/// sweep 将 obj_table 槽置 0 后，通过能否解析指针判断 handle 是否仍指向存活对象。
pub(crate) fn obj_table_handle_live(caller: &mut Caller<'_, RuntimeState>, handle: u32) -> bool {
    resolve_handle_idx(caller, handle as usize).is_some()
}

/// 将 obj_table handle 重新装箱为与堆 header 一致的 NaN-boxed 值（object 或 array）。
pub(crate) fn encode_handle_as_js_value(
    caller: &mut Caller<'_, RuntimeState>,
    handle: u32,
) -> Option<i64> {
    let ptr = resolve_handle_idx(caller, handle as usize)?;
    let env = WasmEnv::from_caller(caller)?;
    let data = env.memory.data(&*caller);
    let heap_type = data.get(ptr + 4).copied()?;
    Some(if heap_type == wjsm_ir::HEAP_TYPE_ARRAY {
        value::encode_handle(value::TAG_ARRAY, handle)
    } else {
        value::encode_object_handle(handle)
    })
}

/// 通过 handle 表解析 boxed value 的真实对象指针。
/// 函数值低 32 位是函数表索引；函数属性对象 handle 从 __function_props_base 起算。
pub(crate) fn resolve_handle(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Option<usize> {
    let handle_idx = handle_index_of(caller, val);
    resolve_handle_idx(caller, handle_idx)
}

/// SameValueZero (ECMAScript §7.2.12)：Map/Set 等键比较；NaN 与 NaN、+0 与 -0 视为相等。
pub(crate) fn same_value_zero(caller: &Caller<'_, RuntimeState>, a: i64, b: i64) -> bool {
    if a == b {
        return true;
    }
    let a_type = type_tag(a);
    let b_type = type_tag(b);
    if a_type != b_type {
        return false;
    }
    match a_type {
        0 => {
            let fa = value::decode_f64(a);
            let fb = value::decode_f64(b);
            if fa.is_nan() && fb.is_nan() {
                return true;
            }
            if fa == 0.0 && fb == 0.0 {
                return true;
            }
            fa == fb
        }
        1 => {
            // 字符串内容比较：runtime_string handle 比较，string_ptr 退回为值比较
            if value::is_runtime_string_handle(a) && value::is_runtime_string_handle(b) {
                let ha = value::decode_runtime_string_handle(a) as usize;
                let hb = value::decode_runtime_string_handle(b) as usize;
                let strings = caller
                    .data()
                    .runtime_strings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                strings
                    .get(ha)
                    .zip(strings.get(hb))
                    .map(|(x, y)| x == y)
                    .unwrap_or(false)
            } else {
                // string_ptr 字面量：相同内容共享同一指针
                a == b
            }
        }
        6 => {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let table = caller
                .data()
                .bigint_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table
                .get(a_handle)
                .zip(table.get(b_handle))
                .map(|(x, y)| x == y)
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// 通过 handle_idx 解析真实对象指针。
///
/// ZGC 会在 `obj_table` entry 低位保存颜色，host 侧解引用必须复用
/// `heap_access` 的统一 load-barrier 入口，避免把带色指针当作线性内存地址。
pub(crate) fn resolve_handle_idx_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    handle_idx: usize,
) -> Option<usize> {
    let handle = u32::try_from(handle_idx).ok()?;
    Some(crate::runtime_gc::heap_access::resolve(ctx, env, handle)?.ptr)
}

// ── Array helpers ──────────────────────────────────────────────────────

/// 解析 TAG_ARRAY 值 → 数组对象的内存指针
pub(crate) fn resolve_array_ptr_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    val: i64,
) -> Option<usize> {
    let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
    resolve_handle_idx_with_env(ctx, env, handle_idx)
}

/// 读取数组的 length 字段（offset 8）
pub(crate) fn read_array_length_with_env<C: AsContext>(
    ctx: &C,
    env: &WasmEnv,
    ptr: usize,
) -> Option<u32> {
    let d = env.memory.data(ctx);
    if ptr + 16 > d.len() {
        return None;
    }
    Some(u32::from_le_bytes([
        d[ptr + 8],
        d[ptr + 9],
        d[ptr + 10],
        d[ptr + 11],
    ]))
}

pub(crate) fn write_array_length_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    ptr: usize,
    len: u32,
) {
    let d = env.memory.data_mut(&mut *ctx);
    if ptr + 16 > d.len() {
        return;
    }
    d[ptr + 8..ptr + 12].copy_from_slice(&len.to_le_bytes());
}

/// 读取数组的 capacity 字段（offset 12）
pub(crate) fn read_array_capacity(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
) -> Option<u32> {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return None;
    };
    let d = mem.data(&*caller);
    if ptr + 16 > d.len() {
        return None;
    }
    Some(u32::from_le_bytes([
        d[ptr + 12],
        d[ptr + 13],
        d[ptr + 14],
        d[ptr + 15],
    ]))
}

/// 读取数组原始槽位值（hole sentinel 保持原样）
pub(crate) fn read_array_elem_raw_with_env<C: AsContext>(
    ctx: &C,
    env: &WasmEnv,
    ptr: usize,
    index: u32,
) -> Option<i64> {
    let d = env.memory.data(ctx);
    let elem_offset = ptr + 16 + (index as usize) * 8;
    if elem_offset + 8 > d.len() {
        return None;
    }
    Some(i64::from_le_bytes([
        d[elem_offset],
        d[elem_offset + 1],
        d[elem_offset + 2],
        d[elem_offset + 3],
        d[elem_offset + 4],
        d[elem_offset + 5],
        d[elem_offset + 6],
        d[elem_offset + 7],
    ]))
}

/// 读取数组元素；hole 视为缺失，返回 None。
pub(crate) fn read_array_elem_with_env<C: AsContext>(
    ctx: &C,
    env: &WasmEnv,
    ptr: usize,
    index: u32,
) -> Option<i64> {
    let value = read_array_elem_raw_with_env(ctx, env, ptr, index)?;
    if value::is_array_hole(value) {
        None
    } else {
        Some(value)
    }
}

pub(crate) fn array_elem_present_with_env<C: AsContext>(
    ctx: &C,
    env: &WasmEnv,
    ptr: usize,
    index: u32,
) -> bool {
    read_array_elem_raw_with_env(ctx, env, ptr, index)
        .is_some_and(|value| !value::is_array_hole(value))
}

/// 写入数组元素
pub(crate) fn write_array_elem_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    ptr: usize,
    index: u32,
    val: i64,
) {
    let _ =
        crate::runtime_gc::heap_access::write_element_at_ptr(ctx, env, ptr, index as usize, val);
}

pub(crate) fn write_array_hole_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    ptr: usize,
    index: u32,
) {
    write_array_elem_with_env(ctx, env, ptr, index, value::encode_array_hole());
}

/// 数组动态扩容 — 遵循现有对象扩容的 capacity × 2 倍增策略
pub(crate) fn grow_array(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
    this_val: i64,
    new_cap: u32,
) -> Option<usize> {
    let env = WasmEnv::from_caller(caller)?;
    let new_size = 16 + new_cap as usize * 8;
    // handle_idx 必须在 data_mut 借用前计算（handle_index_of 需 &mut caller）
    let handle_idx = handle_index_of(caller, this_val);
    let old_size = {
        let cap = read_array_capacity(caller, ptr)?;
        16 + cap as usize * 8
    };
    let heap_ptr = crate::runtime_heap::alloc_heap_region_for_host(
        caller,
        &env,
        new_size,
        wjsm_ir::HEAP_TYPE_ARRAY,
        new_cap,
    )?;
    let ptr = resolve_handle_idx_with_env(caller, &env, handle_idx)?;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *caller).i32().unwrap_or(0) as usize;
    let d = env.memory.data_mut(&mut *caller);
    d.copy_within(ptr..ptr + old_size, heap_ptr);
    d[heap_ptr + 12..heap_ptr + 16].copy_from_slice(&new_cap.to_le_bytes());
    let slot_addr = obj_table_ptr + handle_idx * 4;
    if slot_addr + 4 <= d.len() {
        d[slot_addr..slot_addr + 4].copy_from_slice(&(heap_ptr as u32).to_le_bytes());
    }
    // 注册被抛弃的旧区域（P4-blocker #1）：handle 现在指向 heap_ptr，
    // 旧 ptr 区域不再被 obj_table 索引，sweep 单独遍历看不到 → 注册供 sweeper 回收。
    caller.data().abandon_region(ptr, old_size);
    Some(heap_ptr)
}
/// 对象动态扩容 — 遵循 capacity × 2 倍增策略，与 grow_array 同构
/// 对象槽位大小为 32 bytes（name_id:4 + flags:4 + value:8 + reserved:16）
pub(crate) fn grow_object(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
    handle_val: i64,
    new_cap: u32,
) -> Option<usize> {
    let env = WasmEnv::from_caller(caller)?;
    let new_size = 16 + new_cap as usize * 32;
    // handle_idx 必须在 data_mut 借用前计算（handle_index_of 需 &mut caller）
    let handle_idx = handle_index_of(caller, handle_val);
    let old_cap = {
        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
            return None;
        };
        let d = mem.data(&*caller);
        if ptr + 12 > d.len() {
            return None;
        }
        u32::from_le_bytes([d[ptr + 8], d[ptr + 9], d[ptr + 10], d[ptr + 11]]) as usize
    };
    let old_size = 16 + old_cap * 32;
    let heap_ptr = crate::runtime_heap::alloc_heap_region_for_host(
        caller,
        &env,
        new_size,
        wjsm_ir::HEAP_TYPE_OBJECT,
        new_cap,
    )?;
    let ptr = resolve_handle_idx_with_env(caller, &env, handle_idx)?;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *caller).i32().unwrap_or(0) as usize;
    let d = env.memory.data_mut(&mut *caller);
    d.copy_within(ptr..ptr + old_size, heap_ptr);
    d[heap_ptr + 8..heap_ptr + 12].copy_from_slice(&new_cap.to_le_bytes());
    let slot_addr = obj_table_ptr + handle_idx * 4;
    if slot_addr + 4 <= d.len() {
        d[slot_addr..slot_addr + 4].copy_from_slice(&(heap_ptr as u32).to_le_bytes());
    }
    // 注册被抛弃的旧区域（P4-blocker #1）：同 grow_array。
    caller.data().abandon_region(ptr, old_size);
    Some(heap_ptr)
}

/// 沿原型链递归查找属性（带 visited set 防环路）
pub(crate) fn read_object_property_by_name_proto_walk_with_env<
    C: AsContextMut<Data = RuntimeState>,
>(
    ctx: &mut C,
    env: &WasmEnv,
    obj_ptr: usize,
    prop_name: &str,
    visited: &mut std::collections::HashSet<usize>,
) -> Option<i64> {
    if !visited.insert(obj_ptr) {
        return None; // 环路检测
    }
    let num_props = {
        let data = env.memory.data(&*ctx);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]) as usize
    };
    let mut slots = Vec::with_capacity(num_props);
    {
        let data = env.memory.data(&*ctx);
        for i in 0..num_props {
            let slot_offset = obj_ptr + 16 + i * 32;
            if slot_offset + 8 > data.len() {
                break;
            }
            let name_id = u32::from_le_bytes([
                data[slot_offset],
                data[slot_offset + 1],
                data[slot_offset + 2],
                data[slot_offset + 3],
            ]);
            let flags = i32::from_le_bytes([
                data[slot_offset + 4],
                data[slot_offset + 5],
                data[slot_offset + 6],
                data[slot_offset + 7],
            ]);
            slots.push((name_id, flags));
        }
    }
    for (i, (name_id, flags)) in slots.iter().enumerate() {
        if (*flags & constants::FLAG_PRIVATE) != 0 || is_symbol_name_id(*name_id) {
            continue;
        }
        let name_bytes = read_string_bytes_mem(ctx, &env.memory, *name_id);
        if name_bytes == prop_name.as_bytes() {
            let data = env.memory.data(&*ctx);
            let slot_offset = obj_ptr + 16 + i * 32;
            if slot_offset + 32 > data.len() {
                return None;
            }
            return Some(i64::from_le_bytes([
                data[slot_offset + 8],
                data[slot_offset + 9],
                data[slot_offset + 10],
                data[slot_offset + 11],
                data[slot_offset + 12],
                data[slot_offset + 13],
                data[slot_offset + 14],
                data[slot_offset + 15],
            ]));
        }
    }
    // 自身未找到 → 继续沿原型链
    let proto_handle = {
        let data = env.memory.data(&*ctx);
        if obj_ptr + 4 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr],
            data[obj_ptr + 1],
            data[obj_ptr + 2],
            data[obj_ptr + 3],
        ])
    };
    if proto_handle == 0xFFFF_FFFF || proto_handle == 0 {
        return None;
    }
    let proto_ptr = resolve_handle_idx_with_env(ctx, env, proto_handle as usize)?;
    read_object_property_by_name_proto_walk_with_env(ctx, env, proto_ptr, prop_name, visited)
}

/// 从对象中按名称读取属性值（用于 define_property 等）
pub(crate) fn read_object_property_by_name_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    obj_ptr: usize,
    prop_name: &str,
) -> Option<i64> {
    let num_props = {
        let data = env.memory.data(&*ctx);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]) as usize
    };
    let prop_key = RuntimeString::from_utf8_str(prop_name);
    let mut slots = Vec::with_capacity(num_props);
    {
        let data = env.memory.data(&*ctx);
        for i in 0..num_props {
            let slot_offset = obj_ptr + 16 + i * 32;
            if slot_offset + 8 > data.len() {
                break;
            }
            let name_id = u32::from_le_bytes([
                data[slot_offset],
                data[slot_offset + 1],
                data[slot_offset + 2],
                data[slot_offset + 3],
            ]);
            let flags = i32::from_le_bytes([
                data[slot_offset + 4],
                data[slot_offset + 5],
                data[slot_offset + 6],
                data[slot_offset + 7],
            ]);
            slots.push((name_id, flags));
        }
    }
    for (i, (name_id, flags)) in slots.iter().enumerate() {
        if (*flags & constants::FLAG_PRIVATE) != 0 || is_symbol_name_id(*name_id) {
            continue;
        }
        if name_id_matches_runtime_string(ctx, env, *name_id, &prop_key) {
            let data = env.memory.data(&*ctx);
            let slot_offset = obj_ptr + 16 + i * 32;
            if slot_offset + 32 > data.len() {
                return None;
            }
            return Some(i64::from_le_bytes([
                data[slot_offset + 8],
                data[slot_offset + 9],
                data[slot_offset + 10],
                data[slot_offset + 11],
                data[slot_offset + 12],
                data[slot_offset + 13],
                data[slot_offset + 14],
                data[slot_offset + 15],
            ]));
        }
    }
    // 自身属性未找到 → 沿 [[Prototype]] 链查找
    let proto_handle = {
        let data = env.memory.data(&*ctx);
        if obj_ptr + 4 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr],
            data[obj_ptr + 1],
            data[obj_ptr + 2],
            data[obj_ptr + 3],
        ])
    };
    if proto_handle == 0xFFFF_FFFF || proto_handle == 0 {
        return None;
    }
    let proto_ptr = resolve_handle_idx_with_env(ctx, env, proto_handle as usize)?;
    let mut visited: std::collections::HashSet<usize> = std::collections::HashSet::new();
    visited.insert(obj_ptr);
    read_object_property_by_name_proto_walk_with_env(ctx, env, proto_ptr, prop_name, &mut visited)
}

fn find_property_slot_by_name_id_with_visibility<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    obj_ptr: usize,
    name_id: u32,
    private_slot: bool,
) -> Option<(usize, i32, i64)> {
    let num_props = {
        let data = env.memory.data(&*ctx);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]) as usize
    };
    let target_key = match decode_name_id(name_id) {
        DecodedNameId::MemoryString(index) => Some(RuntimeString::from_utf8_lossy(
            &read_string_bytes_mem(ctx, &env.memory, index),
        )),
        DecodedNameId::RuntimeString(index) => {
            runtime_property_key_units(ctx.as_context().data(), index)
        }
        DecodedNameId::Symbol(_) => None,
    };
    for i in 0..num_props {
        let slot_offset = obj_ptr + 16 + i * 32;
        let (slot_name_id, flags, val) = {
            let data = env.memory.data(&*ctx);
            if slot_offset + 32 > data.len() {
                break;
            }
            let slot_name_id = u32::from_le_bytes([
                data[slot_offset],
                data[slot_offset + 1],
                data[slot_offset + 2],
                data[slot_offset + 3],
            ]);
            let flags = i32::from_le_bytes([
                data[slot_offset + 4],
                data[slot_offset + 5],
                data[slot_offset + 6],
                data[slot_offset + 7],
            ]);
            let val = i64::from_le_bytes([
                data[slot_offset + 8],
                data[slot_offset + 9],
                data[slot_offset + 10],
                data[slot_offset + 11],
                data[slot_offset + 12],
                data[slot_offset + 13],
                data[slot_offset + 14],
                data[slot_offset + 15],
            ]);
            (slot_name_id, flags, val)
        };
        if ((flags & constants::FLAG_PRIVATE) != 0) != private_slot {
            continue;
        }
        let same_name = slot_name_id == name_id
            || target_key
                .as_ref()
                .is_some_and(|key| name_id_matches_runtime_string(ctx, env, slot_name_id, key));
        if same_name {
            return Some((slot_offset, flags, val));
        }
    }
    None
}

/// 从对象中按 name_id 查找普通属性的 slot_offset。
pub(crate) fn find_property_slot_by_name_id_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    obj_ptr: usize,
    name_id: u32,
) -> Option<(usize, i32, i64)> {
    find_property_slot_by_name_id_with_visibility(ctx, env, obj_ptr, name_id, false)
}

/// 从对象中按 name_id 查找类私有成员槽。
pub(crate) fn find_private_property_slot_by_name_id_with_env<
    C: AsContextMut<Data = RuntimeState>,
>(
    ctx: &mut C,
    env: &WasmEnv,
    obj_ptr: usize,
    name_id: u32,
) -> Option<(usize, i32, i64)> {
    find_property_slot_by_name_id_with_visibility(ctx, env, obj_ptr, name_id, true)
}

pub(crate) fn read_object_property_by_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    name_id: u32,
) -> Option<i64> {
    let env = WasmEnv::from_caller(caller)?;
    let (slot_offset, _flags, val) =
        find_property_slot_by_name_id_with_env(caller, &env, obj_ptr, name_id)?;
    let _ = slot_offset;
    Some(val)
}

pub(crate) fn read_iterator_method(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
) -> Option<i64> {
    let method = read_object_property_by_name_id(caller, obj_ptr, encode_symbol_name_id(0))
        .or_else(|| read_object_property_by_name(caller, obj_ptr, "Symbol.iterator"))?;
    if value::is_callable(method) {
        Some(method)
    } else {
        None
    }
}

pub(crate) fn write_object_property_by_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    obj_handle: i64,
    name_id: u32,
    val: i64,
    flags: i32,
) {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let handle = handle_index_of(caller, obj_handle) as u32;
    let found = find_property_slot_by_name_id_with_env(caller, &env, obj_ptr, name_id);
    if let Some((slot_offset, _, _)) = found {
        let slot_idx = (slot_offset - (obj_ptr + 16)) / 32;
        let _ = crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle,
            slot_idx,
            crate::runtime_gc::heap_access::SlotPart::Value,
            val,
        );
        let _ = flags;
        return;
    }

    let (num_props, capacity) = {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return;
        };
        let data = memory.data(&*caller);
        if obj_ptr + 16 > data.len() {
            return;
        }
        let cap = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]) as usize;
        let num = u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]) as usize;
        (num, cap)
    };
    if num_props >= capacity {
        let Some(new_cap) = grown_object_capacity(capacity, 4) else {
            return;
        };
        if grow_object(caller, obj_ptr, obj_handle, new_cap).is_none() {
            return;
        }
    }
    let Some(actual_ptr) = resolve_handle_idx_with_env(caller, &env, handle as usize) else {
        return;
    };
    let num_props = {
        let data = env.memory.data(&*caller);
        if actual_ptr + 16 > data.len() {
            return;
        }
        u32::from_le_bytes([
            data[actual_ptr + 12],
            data[actual_ptr + 13],
            data[actual_ptr + 14],
            data[actual_ptr + 15],
        ]) as usize
    };
    let new_count = num_props + 1;
    let slot_idx = num_props;
    let slot_offset = actual_ptr + 16 + slot_idx * 32;
    {
        let data = env.memory.data_mut(&mut *caller);
        if slot_offset + 32 > data.len() {
            return;
        }
        data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
        data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
    }
    let _ = crate::runtime_gc::heap_access::write_property_slot(
        caller,
        &env,
        handle,
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Value,
        val,
    );
    let data = env.memory.data_mut(&mut *caller);
    if actual_ptr + 16 <= data.len() {
        data[actual_ptr + 12..actual_ptr + 16].copy_from_slice(&(new_count as u32).to_le_bytes());
    }
}

/// 在对象上安装或合并私有访问器槽（ES 类私有 getter/setter）。
pub(crate) fn write_private_accessor_slot(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    obj_handle: i64,
    name_id: u32,
    getter: i64,
    setter: i64,
) {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let handle = handle_index_of(caller, obj_handle) as u32;
    let undef = value::encode_undefined();
    let accessor_flags = constants::FLAG_PRIVATE | constants::FLAG_IS_ACCESSOR;
    if let Some((slot_offset, flags, _)) =
        find_private_property_slot_by_name_id_with_env(caller, &env, obj_ptr, name_id)
    {
        let slot_idx = (slot_offset - (obj_ptr + 16)) / 32;
        if (flags & constants::FLAG_IS_ACCESSOR) != 0 {
            if !value::is_undefined(getter) {
                let _ = crate::runtime_gc::heap_access::write_property_slot(
                    caller,
                    &env,
                    handle,
                    slot_idx,
                    crate::runtime_gc::heap_access::SlotPart::Getter,
                    getter,
                );
            }
            if !value::is_undefined(setter) {
                let _ = crate::runtime_gc::heap_access::write_property_slot(
                    caller,
                    &env,
                    handle,
                    slot_idx,
                    crate::runtime_gc::heap_access::SlotPart::Setter,
                    setter,
                );
            }
        } else {
            {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return;
                };
                let data = memory.data_mut(&mut *caller);
                if slot_offset + 32 > data.len() {
                    return;
                }
                data[slot_offset + 4..slot_offset + 8]
                    .copy_from_slice(&accessor_flags.to_le_bytes());
            }
            let _ = crate::runtime_gc::heap_access::write_property_slot(
                caller,
                &env,
                handle,
                slot_idx,
                crate::runtime_gc::heap_access::SlotPart::Value,
                undef,
            );
            let _ = crate::runtime_gc::heap_access::write_property_slot(
                caller,
                &env,
                handle,
                slot_idx,
                crate::runtime_gc::heap_access::SlotPart::Getter,
                getter,
            );
            let _ = crate::runtime_gc::heap_access::write_property_slot(
                caller,
                &env,
                handle,
                slot_idx,
                crate::runtime_gc::heap_access::SlotPart::Setter,
                setter,
            );
        }
        return;
    }

    let (capacity, num_props) = {
        let data = env.memory.data(&*caller);
        if obj_ptr + 16 > data.len() {
            return;
        }
        let cap = u32::from_le_bytes([
            data[obj_ptr + 8],
            data[obj_ptr + 9],
            data[obj_ptr + 10],
            data[obj_ptr + 11],
        ]) as usize;
        let num = u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]) as usize;
        (cap, num)
    };
    if num_props >= capacity {
        let Some(new_cap) = grown_object_capacity(capacity, 4) else {
            return;
        };
        if grow_object(caller, obj_ptr, obj_handle, new_cap).is_none() {
            return;
        }
    }
    let Some(actual_ptr) = resolve_handle_idx_with_env(caller, &env, handle as usize) else {
        return;
    };
    let num_props = {
        let data = env.memory.data(&*caller);
        if actual_ptr + 16 > data.len() {
            return;
        }
        u32::from_le_bytes([
            data[actual_ptr + 12],
            data[actual_ptr + 13],
            data[actual_ptr + 14],
            data[actual_ptr + 15],
        ]) as usize
    };
    let slot_idx = num_props;
    let slot_offset = actual_ptr + 16 + slot_idx * 32;
    let g = if value::is_undefined(getter) {
        undef
    } else {
        getter
    };
    let s = if value::is_undefined(setter) {
        undef
    } else {
        setter
    };
    {
        let data = env.memory.data_mut(&mut *caller);
        if slot_offset + 32 > data.len() {
            return;
        }
        data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
        data[slot_offset + 4..slot_offset + 8].copy_from_slice(&accessor_flags.to_le_bytes());
    }
    let _ = crate::runtime_gc::heap_access::write_property_slot(
        caller,
        &env,
        handle,
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Value,
        undef,
    );
    let _ = crate::runtime_gc::heap_access::write_property_slot(
        caller,
        &env,
        handle,
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Getter,
        g,
    );
    let _ = crate::runtime_gc::heap_access::write_property_slot(
        caller,
        &env,
        handle,
        slot_idx,
        crate::runtime_gc::heap_access::SlotPart::Setter,
        s,
    );
    let data = env.memory.data_mut(&mut *caller);
    if actual_ptr + 16 <= data.len() {
        data[actual_ptr + 12..actual_ptr + 16]
            .copy_from_slice(&((num_props + 1) as u32).to_le_bytes());
    }
}

/// 读取对象/函数的所有属性名，用于 for...in 枚举
pub(crate) fn enumerate_object_keys(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
) -> Vec<String> {
    if value::is_array(val) {
        return collect_own_property_names_from_value(caller, val, true);
    }

    // 解析对象指针：通过 handle 表统一解析
    let ptr: usize = match resolve_handle(caller, val) {
        Some(p) => p,
        None => return Vec::new(),
    };

    // 读取属性列表
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Vec::new();
    };
    let data = memory.data(&*caller);
    if ptr + 16 > data.len() {
        return Vec::new();
    }

    let num_props = u32::from_le_bytes([
        data[ptr + 12],
        data[ptr + 13],
        data[ptr + 14],
        data[ptr + 15],
    ]) as usize;

    let mut name_ids = Vec::with_capacity(num_props);
    for i in 0..num_props {
        let slot_offset = ptr + 16 + i * 32;
        if slot_offset + 8 > data.len() {
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
        name_ids.push(name_id);
    }
    let _ = data; // 释放对 memory 的借用

    let mut keys = Vec::with_capacity(name_ids.len());
    for name_id in name_ids {
        if is_symbol_name_id(name_id) {
            continue;
        }
        let name_bytes = read_string_bytes(caller, name_id);
        if let Ok(name) = std::str::from_utf8(&name_bytes) {
            keys.push(name.to_string());
        }
    }
    keys
}

/// 分配描述符对象，用于 Object.getOwnPropertyDescriptor 返回值
/// 对象格式：header(16 bytes) + 4 slots * 32 bytes = 144 bytes
#[allow(clippy::too_many_arguments)]
pub(crate) fn allocate_descriptor_object(
    caller: &mut Caller<'_, RuntimeState>,
    is_accessor: bool,
    value: i64,
    writable: bool,
    enumerable: bool,
    configurable: bool,
    getter: i64,
    setter: i64,
) -> Option<i64> {
    let env = WasmEnv::from_caller(caller)?;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *caller).i32().unwrap_or(0) as usize;
    let obj_table_count = env.obj_table_count.get(&mut *caller).i32().unwrap_or(0) as usize;

    // 对象大小：16 (header) + 4 * 32 (slots) = 144 bytes
    let obj_size = 16 + 4 * 32;
    let handle_idx = obj_table_count;

    let heap_ptr = crate::runtime_heap::alloc_heap_region_for_host(
        caller,
        &env,
        obj_size,
        wjsm_ir::HEAP_TYPE_OBJECT,
        4,
    )?;
    crate::runtime_gc::heap_access::init_proto_at_ptr(caller, &env, heap_ptr, 0u32)?;
    {
        let data = env.memory.data_mut(&mut *caller);

        // 初始化 header: proto=0, type=OBJECT, pad=0, capacity=4, num_props=0
        // proto initialized through heap_access owner
        data[heap_ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT; // type byte
        data[heap_ptr + 5..heap_ptr + 8].fill(0); // pad bytes
        data[heap_ptr + 8..heap_ptr + 12].copy_from_slice(&4u32.to_le_bytes()); // capacity
        data[heap_ptr + 12..heap_ptr + 16].copy_from_slice(&0u32.to_le_bytes()); // num_props

        // 注册到 handle 表
        let slot_addr = obj_table_ptr + handle_idx * 4;
        if slot_addr + 4 <= data.len() {
            data[slot_addr..slot_addr + 4].copy_from_slice(&(heap_ptr as u32).to_le_bytes());
        }
    }

    {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") else {
            return None;
        };
        let _ = g.set(&mut *caller, Val::I32((handle_idx + 1) as i32));
    }

    // 现在设置描述符对象的属性
    let desc_ptr = heap_ptr;

    // 写入属性的辅助闭包
    let mut write_property = |name_id: u32, val: i64, flags: i32| -> Option<()> {
        let num_props = {
            let data = env.memory.data(&*caller);
            u32::from_le_bytes([
                data[desc_ptr + 12],
                data[desc_ptr + 13],
                data[desc_ptr + 14],
                data[desc_ptr + 15],
            ]) as usize
        };
        let slot_offset = desc_ptr + 16 + num_props * 32;
        {
            let data = env.memory.data_mut(&mut *caller);
            if slot_offset + 32 > data.len() {
                return None;
            }
            data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
            data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
        }
        let undef = value::encode_undefined();
        crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle_idx as u32,
            num_props,
            crate::runtime_gc::heap_access::SlotPart::Value,
            val,
        )?;
        crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle_idx as u32,
            num_props,
            crate::runtime_gc::heap_access::SlotPart::Getter,
            undef,
        )?;
        crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle_idx as u32,
            num_props,
            crate::runtime_gc::heap_access::SlotPart::Setter,
            undef,
        )?;
        let data = env.memory.data_mut(&mut *caller);
        let new_num_props = (num_props + 1) as u32;
        data[desc_ptr + 12..desc_ptr + 16].copy_from_slice(&new_num_props.to_le_bytes());
        Some(())
    };

    // flags: enumerable 和 configurable
    let base_flags: i32 =
        (if enumerable { 1 << 1 } else { 0 }) | (if configurable { 1 } else { 0 });

    if is_accessor {
        // 访问器属性：get, set, enumerable, configurable
        // writable flag 不适用于访问器属性
        let get_flags = base_flags | (1 << 2); // writable=true for function values
        write_property(constants::PROP_DESC_GET_OFFSET, getter, get_flags)?;
        write_property(constants::PROP_DESC_SET_OFFSET, setter, get_flags)?;
    } else {
        // 数据属性：value, writable, enumerable, configurable
        let writable_flags = base_flags | (if writable { 1 << 2 } else { 0 });
        write_property(constants::PROP_DESC_VALUE_OFFSET, value, writable_flags)?;
        write_property(
            constants::PROP_DESC_WRITABLE_OFFSET,
            value::encode_bool(writable),
            base_flags | (1 << 2),
        )?;
    }

    // enumerable 和 configurable 对于两种属性都要写
    write_property(
        constants::PROP_DESC_ENUMERABLE_OFFSET,
        value::encode_bool(enumerable),
        base_flags | (1 << 2),
    )?;
    write_property(
        constants::PROP_DESC_CONFIGURABLE_OFFSET,
        value::encode_bool(configurable),
        base_flags | (1 << 2),
    )?;

    // 返回对象 handle
    Some(value::encode_object_handle(handle_idx as u32))
}

// ── 辅助函数用于 abstract_eq 和 abstract_compare ─────────────────────────

/// ECMAScript §7.1.1 ToPrimitive hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToPrimitiveHint {
    Default,
    Number,
    String,
}

const WELL_KNOWN_SYMBOL_TO_PRIMITIVE: u32 = 5;

fn to_primitive_hint_string(hint: ToPrimitiveHint) -> String {
    match hint {
        ToPrimitiveHint::String => "string".to_string(),
        ToPrimitiveHint::Number => "number".to_string(),
        ToPrimitiveHint::Default => "default".to_string(),
    }
}

fn is_object_like(val: i64) -> bool {
    value::is_object(val) || value::is_callable(val) || value::is_function(val)
}

fn read_object_method(caller: &mut Caller<'_, RuntimeState>, obj: i64, name: &str) -> Option<i64> {
    let ptr = resolve_handle(caller, obj)?;
    read_object_property_by_name(caller, ptr, name)
}

fn invoke_to_primitive_method_sync(
    caller: &mut Caller<'_, RuntimeState>,
    func: i64,
    this_val: i64,
    hint: ToPrimitiveHint,
) -> i64 {
    let hint_arg = store_runtime_string(caller, to_primitive_hint_string(hint));
    if value::is_native_callable(func) {
        return call_native_callable_with_args_from_caller(caller, func, this_val, vec![hint_arg])
            .unwrap_or_else(value::encode_undefined);
    }
    let rt = tokio::runtime::Handle::current();
    tokio::task::block_in_place(|| {
        rt.block_on(call_wasm_callback_async(
            caller,
            func,
            this_val,
            &[hint_arg],
        ))
    })
    .unwrap_or_else(|_| value::encode_undefined())
}

fn ordinary_to_primitive(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
    hint: ToPrimitiveHint,
) -> i64 {
    let effective_hint = match hint {
        ToPrimitiveHint::Default => ToPrimitiveHint::Number,
        other => other,
    };
    let (first, second) = match effective_hint {
        ToPrimitiveHint::String => ("toString", "valueOf"),
        ToPrimitiveHint::Number => ("valueOf", "toString"),
        ToPrimitiveHint::Default => unreachable!(),
    };
    for method_name in [first, second] {
        let Some(method) = read_object_method(caller, val, method_name) else {
            continue;
        };
        if !is_callable_in_runtime(caller, method) {
            continue;
        }
        let result = invoke_to_primitive_method_sync(caller, method, val, hint);
        if value::is_exception(result) {
            return result;
        }
        if !is_object_like(result) {
            return result;
        }
    }
    make_type_error_exception(
        caller,
        "TypeError: Cannot convert object to primitive value",
    )
}
/// ToBoolean 抽象操作 (ECMAScript 7.1.2)
pub(crate) fn to_boolean(caller: &mut Caller<'_, RuntimeState>, val: i64) -> bool {
    if value::is_undefined(val) || value::is_null(val) {
        return false;
    }
    if value::is_bool(val) {
        return value::decode_bool(val);
    }
    if value::is_f64(val) {
        let f = value::decode_f64(val);
        return f != 0.0 && !f.is_nan();
    }
    if value::is_string(val) {
        if value::is_runtime_string_handle(val) {
            let handle = value::decode_runtime_string_handle(val) as usize;
            let strings = caller
                .data()
                .runtime_strings
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            return !strings.get(handle).map(|s| s.is_empty()).unwrap_or(true);
        }
        let ptr = value::decode_string_ptr(val);
        if let Some(Extern::Memory(memory)) = caller.get_export("memory") {
            let bytes = read_string_bytes_mem(caller, &memory, ptr);
            return !bytes.is_empty();
        }
        return true;
    }
    if value::is_bigint(val) {
        let handle = value::decode_bigint_handle(val) as usize;
        let table = caller
            .data()
            .bigint_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        return table
            .get(handle)
            .map(|bi| *bi != num_bigint::BigInt::from(0))
            .unwrap_or(true);
    }
    // 对象、函数、Symbol、RegExp 等 → truthy
    true
}

/// ToNumber 抽象操作 (ECMAScript 7.1.4)
/// 将值转换为 Number 类型
pub(crate) fn to_number(caller: &mut Caller<'_, RuntimeState>, val: i64) -> i64 {
    // undefined → NaN
    if value::is_undefined(val) {
        return f64::NAN.to_bits() as i64;
    }

    // null → +0
    if value::is_null(val) {
        return 0.0_f64.to_bits() as i64;
    }

    // bool: true → 1, false → 0
    if value::is_bool(val) {
        let b = value::decode_bool(val);
        return (if b { 1.0_f64 } else { 0.0_f64 }).to_bits() as i64;
    }

    // f64 → itself
    if value::is_f64(val) {
        return val;
    }

    // string → parseFloat (可能失败 → NaN)
    if value::is_string(val) {
        let string_lossy = get_string_utf8_lossy(caller, val);
        let num = crate::runtime_string_to_number::js_string_content_to_f64(&string_lossy);
        return num.to_bits() as i64;
    }

    // BigInt → ToNumber: 抛 TypeError (§7.1.4)
    if value::is_bigint(val) {
        *caller
            .data()
            .runtime_error
            .lock()
            .unwrap_or_else(|e| e.into_inner()) =
            Some("TypeError: Cannot convert a BigInt value to a number".to_string());
        return f64::NAN.to_bits() as i64;
    }

    // RegExp → ToNumber: NaN (objects convert to NaN)
    if value::is_regexp(val) {
        return f64::NAN.to_bits() as i64;
    }

    // Symbol → ToNumber: 抛出 TypeError
    if value::is_symbol(val) {
        *caller
            .data()
            .runtime_error
            .lock()
            .unwrap_or_else(|e| e.into_inner()) =
            Some("TypeError: Cannot convert a Symbol value to a number".to_string());
        return f64::NAN.to_bits() as i64;
    }

    // object/function → ToPrimitive(hint: Number) → ToNumber
    if value::is_object(val) || value::is_callable(val) {
        let prim = to_primitive_with_hint(caller, val, ToPrimitiveHint::Number);
        if value::is_exception(prim) {
            return prim;
        }
        return to_number(caller, prim);
    }

    // 其他类型（iterator, enumerator, exception）→ NaN
    f64::NAN.to_bits() as i64
}

/// ToNumeric 抽象操作 (ECMAScript §7.1.16)
/// 对 BigInt 原样返回，否则调用 ToNumber
pub(crate) fn to_numeric(caller: &mut Caller<'_, RuntimeState>, val: i64) -> i64 {
    if value::is_bigint(val) {
        return val;
    }
    to_number(caller, val)
}

/// 比较 ℝ(BigInt) < ℝ(Number)  (§7.2.13 step 5m)
/// bigint_is_left: 调用时 BigInt 是左操作数 (a < b) 还是右操作数 (b < a)
pub(crate) fn number_less_than_bigint(
    num_f: f64,
    bi: &num_bigint::BigInt,
    bigint_is_left: bool,
) -> bool {
    // 将 Number 转换为精确整数（若为整数）后比较
    let truncated = num_f.trunc();
    let is_exact_int = num_f == truncated;

    // Try to get exact integer within BigInt range
    if num_f.is_finite() && (num_f.abs() <= (1i64 << 53) as f64) {
        // Within safe integer range — representable exactly as f64's integer
        let int_val = num_f as i64;
        // Re-check: round-trip must be exact
        if (num_f - (int_val as f64)).abs() < 1.0 {
            let num_bi = num_bigint::BigInt::from(int_val);
            if is_exact_int {
                return if bigint_is_left {
                    *bi < num_bi
                } else {
                    num_bi < *bi
                };
            } else {
                // 带小数：bi 是整数，小数部分让比较略偏向一侧
                return if bigint_is_left {
                    *bi <= num_bi
                } else {
                    num_bi <= *bi
                };
            }
        }
    }

    // Fallback: f64 超出精确整数范围或非整数很大值
    // 用 BigInt 的 to_f64 近似比较
    let bi_f64_op = bi.to_f64();
    let result = match bi_f64_op {
        Some(bi_f64) => {
            if bigint_is_left {
                bi_f64 < num_f
            } else {
                num_f < bi_f64
            }
        }
        None => {
            // BigInt 超出 f64 范围（≳ 2^1024）
            if bi.sign() == num_bigint::Sign::Minus {
                // 极大负数 < 任何有限数 → true
                bigint_is_left
            } else {
                // 极大正数 < 任何有限数 → false
                !bigint_is_left
            }
        }
    };
    result
}

/// ToPrimitive 抽象操作 (ECMAScript §7.1.1)
pub(crate) fn to_primitive(caller: &mut Caller<'_, RuntimeState>, val: i64) -> i64 {
    to_primitive_with_hint(caller, val, ToPrimitiveHint::Default)
}

pub(crate) fn to_primitive_with_hint(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
    hint: ToPrimitiveHint,
) -> i64 {
    if value::is_f64(val)
        || value::is_string(val)
        || value::is_bool(val)
        || value::is_undefined(val)
        || value::is_null(val)
        || value::is_bigint(val)
        || value::is_symbol(val)
    {
        return val;
    }

    if !is_object_like(val) {
        return val;
    }

    if let Some(ptr) = resolve_handle(caller, val) {
        let exotic = read_object_property_by_name_id(
            caller,
            ptr,
            encode_symbol_name_id(WELL_KNOWN_SYMBOL_TO_PRIMITIVE),
        )
        .or_else(|| read_object_property_by_name(caller, ptr, "Symbol.toPrimitive"));
        if let Some(method) = exotic {
            if is_callable_in_runtime(caller, method) {
                let result = invoke_to_primitive_method_sync(caller, method, val, hint);
                if value::is_exception(result) {
                    return result;
                }
                if !is_object_like(result) {
                    return result;
                }
                return make_type_error_exception(
                    caller,
                    "TypeError: Cannot convert object to primitive value",
                );
            }
        }
    }

    ordinary_to_primitive(caller, val, hint)
}

/// ToObject 抽象操作 (ECMAScript 7.1.13)：原始值包装为对象，已是对象则原样返回。
pub(crate) fn to_object(caller: &mut Caller<'_, RuntimeState>, val: i64) -> i64 {
    if value::is_js_object(val) {
        return val;
    }
    if value::is_undefined(val) || value::is_null(val) {
        return val;
    }
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    if value::is_string(val) {
        let s = get_string_value(caller, val);
        let len = s.utf16_len();
        let cap = (len.saturating_add(1)).max(1) as u32;
        let obj = alloc_host_object(caller, &env, cap);
        for (i, unit) in s.as_utf16_units().iter().copied().enumerate() {
            let idx_str = i.to_string();
            let ch_val = store_runtime_string(caller, RuntimeString::from_utf16_code_unit(unit));
            let _ = define_host_data_property_from_caller(caller, obj, &idx_str, ch_val);
        }
        let len_val = value::encode_f64(len as f64);
        let len_name_id = {
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            crate::find_memory_c_string_with_env(caller, &env, "length").unwrap_or(0)
        };
        let _ = crate::define_host_data_property_by_name_id_with_flags(
            caller,
            obj,
            crate::property_key::encode_string_name_id(len_name_id),
            len_val,
            wjsm_ir::constants::FLAG_CONFIGURABLE | wjsm_ir::constants::FLAG_WRITABLE,
        );
        return obj;
    }
    // 其他原始类型：分配空壳对象（Object.assign 等无自有可枚举属性可复制）
    alloc_host_object(caller, &env, 0)
}

pub(crate) fn byte_offset_to_utf16_index(s: &str, byte_off: usize) -> usize {
    let mut utf16_count = 0usize;
    for (off, ch) in s.char_indices() {
        if off >= byte_off {
            break;
        }
        utf16_count += if ch as u32 > 0xFFFF { 2 } else { 1 };
    }
    utf16_count
}

/// 严格相等比较 (ECMAScript 7.2.16)
pub(crate) fn strict_eq(caller: &mut Caller<'_, RuntimeState>, a: i64, b: i64) -> i64 {
    // 类型不同 → false
    let a_type = type_tag(a);
    let b_type = type_tag(b);

    if a_type != b_type {
        return value::encode_bool(false);
    }

    // 同类型比较
    match a_type {
        // f64: 注意 NaN !== NaN
        0 => {
            let af = value::decode_f64(a);
            let bf = value::decode_f64(b);
            if af.is_nan() || bf.is_nan() {
                return value::encode_bool(false);
            }
            value::encode_bool(af == bf)
        }
        // string
        1 => {
            let a_str = get_string_value(caller, a);
            let b_str = get_string_value(caller, b);
            value::encode_bool(a_str == b_str)
        }
        // undefined
        2 => value::encode_bool(true),
        // null
        3 => value::encode_bool(true),
        // bool
        4 => value::encode_bool(value::decode_bool(a) == value::decode_bool(b)),
        // BigInt: 值比较
        6 => {
            let a_handle = value::decode_bigint_handle(a) as usize;
            let b_handle = value::decode_bigint_handle(b) as usize;
            let table = caller
                .data()
                .bigint_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let eq = table
                .get(a_handle)
                .zip(table.get(b_handle))
                .map(|(x, y)| x == y)
                .unwrap_or(false);
            value::encode_bool(eq)
        }
        // Symbol: 引用比较（同一 handle）
        7 => value::encode_bool(a == b),
        // object/function/iterator/enumerator/exception: 引用比较
        _ => {
            // 快速路径：相同 NaN-boxed 值
            if a == b {
                return value::encode_bool(true);
            }
            // TAG_FUNCTION vs TAG_CLOSURE 交叉比较：
            // 闭包的 prototype.constructor 设置为 encode_function_idx(func_idx)（TAG_FUNCTION），
            // 但用户可见的函数值是 TAG_CLOSURE。两者应视为同一函数（ECMAScript 中无区分）。
            let a_is_func = value::is_function(a);
            let b_is_func = value::is_function(b);
            let a_is_closure = value::is_closure(a);
            let b_is_closure = value::is_closure(b);
            if (a_is_func && b_is_closure) || (a_is_closure && b_is_func) {
                let func_idx = if a_is_func { a as u32 } else { b as u32 };
                let closure_idx = if a_is_closure { a as u32 } else { b as u32 };
                let closures = caller
                    .data()
                    .closures
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let eq = closures
                    .get(closure_idx as usize)
                    .map(|c| c.func_idx as u32 == func_idx)
                    .unwrap_or(false);
                return value::encode_bool(eq);
            }
            value::encode_bool(false)
        }
    }
}

/// 获取类型标签 (用于 strict_eq)
/// 返回值: 0=f64, 1=string, 2=undefined, 3=null, 4=bool, 5=object/function/其他, 6=bigint, 7=symbol
pub(crate) fn type_tag(val: i64) -> u64 {
    if value::is_f64(val) {
        0
    } else if value::is_string(val) {
        1
    } else if value::is_undefined(val) {
        2
    } else if value::is_null(val) {
        3
    } else if value::is_bool(val) {
        4
    } else if value::is_bigint(val) {
        6
    } else if value::is_symbol(val) {
        7
    } else {
        5
    } // object, function, iterator, enumerator, exception, bound
}

/// 获取 ECMAScript 字符串值，保留 UTF-16 code units。
pub(crate) fn get_string_value(caller: &mut Caller<'_, RuntimeState>, val: i64) -> RuntimeString {
    if value::is_runtime_string_handle(val) {
        let handle = value::decode_runtime_string_handle(val) as usize;
        let strings = caller
            .data()
            .runtime_strings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        strings.get(handle).cloned().unwrap_or_default()
    } else if value::is_string(val) {
        RuntimeString::from_utf8_lossy(&read_string_bytes(caller, value::decode_string_ptr(val)))
    } else {
        RuntimeString::empty()
    }
}

pub(crate) fn get_string_utf8_lossy(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    get_string_value(caller, val).to_utf8_lossy()
}

pub(crate) async fn resolve_and_call_async(
    caller: &mut Caller<'_, RuntimeState>,
    func: i64,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .unwrap();

    if value::is_bound(func) {
        let bound_idx = value::decode_bound_idx(func);
        let (target_func, bound_this, bound_args_ref) = {
            let bound = caller
                .data()
                .bound_objects
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let record = &bound[bound_idx as usize];
            (
                record.target_func,
                record.bound_this,
                record.bound_args.clone(),
            )
        };

        let total_count = bound_args_ref.len() as i32 + args_count;
        let shadow_sp_global = caller
            .get_export("__shadow_sp")
            .and_then(|e| e.into_global())
            .unwrap();
        let shadow_sp = shadow_sp_global.get(&mut *caller).i32().unwrap();
        // 在当前 shadow_sp 处拼装「bound 前缀 + 调用时参数」。
        // `args_base` 是绝对地址（与 read_shadow_arg 一致），不可再加 shadow_sp。
        let ptr = shadow_sp;

        for (i, arg) in bound_args_ref.iter().enumerate() {
            memory
                .write(
                    &mut *caller,
                    (ptr + i as i32 * 8) as usize,
                    &arg.to_le_bytes(),
                )
                .unwrap();
        }
        for i in 0..args_count {
            let mut buf = [0u8; 8];
            memory
                .read(
                    &mut *caller,
                    (args_base + i * 8) as usize,
                    &mut buf,
                )
                .unwrap();
            memory
                .write(
                    &mut *caller,
                    (ptr + (bound_args_ref.len() as i32 + i) * 8) as usize,
                    &buf,
                )
                .unwrap();
        }

        // 推进 shadow_sp，避免嵌套调用覆盖本帧拼接区。
        let _ = shadow_sp_global.set(&mut *caller, Val::I32(ptr + total_count * 8));

        let result = Box::pin(resolve_callable_and_call_async(
            caller,
            target_func,
            bound_this,
            ptr,
            total_count,
        ))
        .await;

        let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
        result
    } else {
        Box::pin(resolve_callable_and_call_async(
            caller, func, this_val, args_base, args_count,
        ))
        .await
    }
}

pub(crate) async fn resolve_callable_and_call_async(
    caller: &mut Caller<'_, RuntimeState>,
    callee: i64,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (func_idx, env_obj) = if value::is_closure(callee) {
        let idx = value::decode_closure_idx(callee);
        let closures = caller
            .data()
            .closures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = &closures[idx as usize];
        (entry.func_idx, entry.env_obj)
    } else if value::is_function(callee) {
        (
            value::decode_function_idx(callee),
            value::encode_undefined(),
        )
    } else if value::is_bound(callee) {
        return Box::pin(resolve_and_call_async(
            caller, callee, this_val, args_base, args_count,
        ))
        .await;
    } else if value::is_proxy(callee) {
        let handle = value::decode_proxy_handle(callee) as usize;
        let entry = {
            let table = caller
                .data()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Cannot perform 'apply' on a proxy that has been revoked"
                        .to_string(),
                );
                return value::encode_undefined();
            }
            if !is_callable_in_runtime(caller, entry.target) {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Proxy target must be callable".to_string(),
                );
                return value::encode_undefined();
            }
            if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                let trap = read_object_property_by_name(caller, handler_ptr, "apply")
                    .unwrap_or_else(value::encode_undefined);
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    let args_arr =
                        crate::runtime_host_helpers::alloc_array(caller, args_count as u32);
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .unwrap();
                    for i in 0..args_count {
                        let mut buf = [0u8; 8];
                        let _ = memory.read(&mut *caller, (args_base + i * 8) as usize, &mut buf);
                        let arg_val = i64::from_le_bytes(buf);
                        crate::runtime_host_helpers::define_host_data_property(
                            caller,
                            args_arr,
                            &i.to_string(),
                            arg_val,
                        );
                    }
                    return call_wasm_callback_async(
                        caller,
                        trap,
                        entry.handler,
                        &[entry.target, this_val, args_arr],
                    )
                    .await
                    .unwrap_or_else(|_| value::encode_undefined());
                }
            }
            return Box::pin(resolve_callable_and_call_async(
                caller,
                entry.target,
                this_val,
                args_base,
                args_count,
            ))
            .await;
        }
        return value::encode_undefined();
    } else if value::is_native_callable(callee) {
        let memory = caller
            .get_export("memory")
            .and_then(|e| e.into_memory())
            .unwrap();
        let mut collected_args = Vec::with_capacity(args_count as usize);
        for i in 0..args_count {
            let mut buf = [0u8; 8];
            let _ = memory.read(&mut *caller, (args_base + i * 8) as usize, &mut buf);
            collected_args.push(i64::from_le_bytes(buf));
        }
        return call_native_callable_with_args_from_caller_async(
            caller,
            callee,
            this_val,
            collected_args,
        )
        .await
        .unwrap_or_else(value::encode_undefined);
    } else {
        return value::encode_undefined();
    };

    let table = caller
        .get_export("__table")
        .and_then(|e| e.into_table())
        .unwrap();
    let func_ref = table.get(&mut *caller, func_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else {
        return value::encode_undefined();
    };
    let mut results = [Val::I64(0)];
    let _ = func
        .call_async(
            &mut *caller,
            &[
                Val::I64(env_obj),
                Val::I64(this_val),
                Val::I32(args_base),
                Val::I32(args_count),
            ],
            &mut results,
        )
        .await;
    results[0].unwrap_i64()
}

pub(crate) async fn func_apply_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    func: i64,
    this_val: i64,
    args_array: i64,
) -> i64 {
    let args = match crate::host_imports::extract_array_like_elements(caller, args_array).await {
        Ok(v) => v,
        Err(_) => Vec::new(),
    };
    Box::pin(crate::host_imports::reflect_apply_impl_async(
        caller, func, this_val, &args,
    ))
    .await
}

pub(crate) fn func_bind_impl(
    caller: &mut Caller<'_, RuntimeState>,
    func: i64,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .unwrap();
    let mut bound_args = Vec::with_capacity(args_count as usize);
    for i in 0..args_count {
        let mut buf = [0u8; 8];
        memory
            .read(&mut *caller, (args_base + i * 8) as usize, &mut buf)
            .unwrap();
        bound_args.push(i64::from_le_bytes(buf));
    }
    let mut bound = caller
        .data()
        .bound_objects
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let idx = bound.len() as u32;
    bound.push(BoundRecord {
        target_func: func,
        bound_this: this_val,
        bound_args,
    });
    value::encode_bound_idx(idx)
}

pub(crate) fn object_rest_impl(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    excluded_keys: i64,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let Some(source_ptr) = resolve_handle(caller, obj) else {
        return alloc_host_object(caller, &env, 0);
    };

    let excluded_key_bytes = if value::is_array(excluded_keys) {
        let mut excluded = Vec::new();
        if let Some(arr_ptr) = resolve_array_ptr(caller, excluded_keys) {
            let len = read_array_length(caller, arr_ptr).unwrap_or(0);
            for i in 0..len {
                let key =
                    read_array_elem(caller, arr_ptr, i).unwrap_or_else(value::encode_undefined);
                if let Some(bytes) = read_value_string_bytes(caller, key) {
                    excluded.push(bytes);
                }
            }
        }
        excluded
    } else {
        Vec::new()
    };

    let source_props: Vec<(u32, i64)> = {
        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
            return alloc_host_object(caller, &env, 0);
        };
        let num_props = {
            let data = mem.data(&*caller);
            if source_ptr + 16 > data.len() {
                return alloc_host_object(caller, &env, 0);
            }
            u32::from_le_bytes([
                data[source_ptr + 12],
                data[source_ptr + 13],
                data[source_ptr + 14],
                data[source_ptr + 15],
            ]) as usize
        };
        let mut props = Vec::new();
        for i in 0..num_props {
            let slot_offset = source_ptr + 16 + i * 32;
            let (name_id, flags, val) = {
                let data = mem.data(&*caller);
                if slot_offset + 32 > data.len() {
                    break;
                }
                let name_id = u32::from_le_bytes([
                    data[slot_offset],
                    data[slot_offset + 1],
                    data[slot_offset + 2],
                    data[slot_offset + 3],
                ]);
                let flags = i32::from_le_bytes([
                    data[slot_offset + 4],
                    data[slot_offset + 5],
                    data[slot_offset + 6],
                    data[slot_offset + 7],
                ]);
                let val = i64::from_le_bytes([
                    data[slot_offset + 8],
                    data[slot_offset + 9],
                    data[slot_offset + 10],
                    data[slot_offset + 11],
                    data[slot_offset + 12],
                    data[slot_offset + 13],
                    data[slot_offset + 14],
                    data[slot_offset + 15],
                ]);
                (name_id, flags, val)
            };
            if (flags & constants::FLAG_ENUMERABLE) == 0 {
                continue;
            }
            if !excluded_key_bytes.is_empty() {
                let name_bytes = read_string_bytes_mem(caller, &mem, name_id);
                if excluded_key_bytes
                    .iter()
                    .any(|excluded| excluded == &name_bytes)
                {
                    continue;
                }
            }
            props.push((name_id, val));
        }
        props
    };

    let result = alloc_host_object(caller, &env, source_props.len() as u32);
    let Some(result_ptr) = resolve_handle(caller, result) else {
        return result;
    };
    let flags =
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE;
    for (name_id, val) in source_props {
        write_object_property_by_name_id(caller, result_ptr, result, name_id, val, flags);
    }
    result
}

pub(crate) fn obj_spread_impl(caller: &mut Caller<'_, RuntimeState>, dest: i64, source: i64) {
    let Some(mut dest_ptr) = resolve_handle(caller, dest) else {
        return;
    };
    let Some(source_ptr) = resolve_handle(caller, source) else {
        return;
    };

    let source_props: Vec<(u32, i64)> = {
        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
            return;
        };
        let data = mem.data(&*caller);
        if source_ptr + 16 > data.len() {
            return;
        }
        let num_props = u32::from_le_bytes([
            data[source_ptr + 12],
            data[source_ptr + 13],
            data[source_ptr + 14],
            data[source_ptr + 15],
        ]) as usize;
        let mut props = Vec::new();
        for i in 0..num_props {
            let slot_offset = source_ptr + 16 + i * 32;
            if slot_offset + 32 > data.len() {
                break;
            }
            let flags = i32::from_le_bytes([
                data[slot_offset + 4],
                data[slot_offset + 5],
                data[slot_offset + 6],
                data[slot_offset + 7],
            ]);
            if (flags & constants::FLAG_ENUMERABLE) == 0 {
                continue;
            }
            let name_id = u32::from_le_bytes([
                data[slot_offset],
                data[slot_offset + 1],
                data[slot_offset + 2],
                data[slot_offset + 3],
            ]);
            let val = i64::from_le_bytes([
                data[slot_offset + 8],
                data[slot_offset + 9],
                data[slot_offset + 10],
                data[slot_offset + 11],
                data[slot_offset + 12],
                data[slot_offset + 13],
                data[slot_offset + 14],
                data[slot_offset + 15],
            ]);
            props.push((name_id, val));
        }
        props
    };

    let mut new_count = 0usize;
    for (name_id, _) in &source_props {
        if find_property_slot_by_name_id(caller, dest_ptr, *name_id).is_none() {
            new_count += 1;
        }
    }

    if new_count > 0 {
        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
            return;
        };
        let data = mem.data(&*caller);
        if dest_ptr + 16 > data.len() {
            return;
        }
        let capacity = u32::from_le_bytes([
            data[dest_ptr + 8],
            data[dest_ptr + 9],
            data[dest_ptr + 10],
            data[dest_ptr + 11],
        ]) as usize;
        let num_props = u32::from_le_bytes([
            data[dest_ptr + 12],
            data[dest_ptr + 13],
            data[dest_ptr + 14],
            data[dest_ptr + 15],
        ]) as usize;
        let Some(needed_props) = num_props.checked_add(new_count) else {
            return;
        };
        if needed_props > capacity {
            let Some(new_cap) = grown_object_capacity(capacity, needed_props) else {
                return;
            };
            let Some(new_ptr) = grow_object(caller, dest_ptr, dest, new_cap) else {
                return;
            };
            dest_ptr = new_ptr;
        }
    }

    let flags =
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE;
    let Some(env) = WasmEnv::from_caller(caller) else {
        return;
    };
    let dest_handle = handle_index_of(caller, dest) as u32;
    for (name_id, val) in source_props {
        if let Some((slot_offset, _, _)) = find_property_slot_by_name_id(caller, dest_ptr, name_id)
        {
            let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                return;
            };
            {
                let data = mem.data_mut(&mut *caller);
                if slot_offset + 16 > data.len() {
                    return;
                }
                data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
            }
            let slot_idx = (slot_offset - (dest_ptr + 16)) / 32;
            let _ = crate::runtime_gc::heap_access::write_property_slot(
                caller,
                &env,
                dest_handle,
                slot_idx,
                crate::runtime_gc::heap_access::SlotPart::Value,
                val,
            );
        } else {
            write_object_property_by_name_id(caller, dest_ptr, dest, name_id, val, flags);
            if let Some(new_ptr) = resolve_handle(caller, dest) {
                dest_ptr = new_ptr;
            }
        }
    }
}

// ── Caller → _with_env 薄封装宏 ───────────────────────────────────────
macro_rules! caller_env_wrapper {
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty =
            $with_env:ident
    ) => {
        $(#[$meta])*
        $vis fn $name(caller: &mut Caller<'_, RuntimeState>, $($arg: $ty),*) -> $ret {
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            $with_env(caller, &env, $($arg),*)
        }
    };
    // 变体：返回单元类型（无 -> $ret）
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($($arg:ident: $ty:ty),*) =
            $with_env:ident
    ) => {
        $(#[$meta])*
        $vis fn $name(caller: &mut Caller<'_, RuntimeState>, $($arg: $ty),*) {
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            $with_env(caller, &env, $($arg),*)
        }
    };
}

caller_env_wrapper! {
    pub(crate) fn resolve_handle_idx(handle_idx: usize) -> Option<usize> = resolve_handle_idx_with_env
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn resolve_array_ptr(val: i64) -> Option<usize> = resolve_array_ptr_with_env
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn read_array_length(ptr: usize) -> Option<u32> = read_array_length_with_env
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn write_array_length(ptr: usize, len: u32) = write_array_length_with_env
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn read_array_elem(ptr: usize, index: u32) -> Option<i64> = read_array_elem_with_env
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn write_array_elem(ptr: usize, index: u32, val: i64) = write_array_elem_with_env
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn array_elem_present(ptr: usize, index: u32) -> bool = array_elem_present_with_env
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn write_array_hole(ptr: usize, index: u32) = write_array_hole_with_env
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn read_object_property_by_name(obj_ptr: usize, prop_name: &str) -> Option<i64> = read_object_property_by_name_with_env
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn read_object_property_by_name_proto_walk(obj_ptr: usize, prop_name: &str, visited: &mut std::collections::HashSet<usize>) -> Option<i64> = read_object_property_by_name_proto_walk_with_env
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn find_property_slot_by_name_id(obj_ptr: usize, name_id: u32) -> Option<(usize, i32, i64)> = find_property_slot_by_name_id_with_env
}

caller_env_wrapper! {
    #[inline]
    pub(crate) fn find_private_property_slot_by_name_id(obj_ptr: usize, name_id: u32) -> Option<(usize, i32, i64)> = find_private_property_slot_by_name_id_with_env
}
