use super::*;
use crate::runtime_heap::{alloc_heap_region_for_host, host_handle_slot_fits};
use std::sync::atomic::Ordering;
use wjsm_ir::constants;

/// Well-known `Symbol.species` table index (see `RuntimeState::symbol_table` init order).
const WELL_KNOWN_SYMBOL_SPECIES: u32 = 1;

fn default_array_constructor(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    create_native_callable(caller.data(), NativeCallable::ArrayConstructor)
}

fn is_native_array_constructor(caller: &mut Caller<'_, RuntimeState>, constructor: i64) -> bool {
    if !value::is_native_callable(constructor) {
        return false;
    }
    let idx = value::decode_native_callable_idx(constructor) as usize;
    let table = caller
        .data()
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    matches!(table.get(idx), Some(NativeCallable::ArrayConstructor))
}

/// ES2024 `SpeciesConstructor(O, %Array%)` for Array exotic objects.
pub(crate) fn array_species_constructor(
    caller: &mut Caller<'_, RuntimeState>,
    exemplar: i64,
) -> i64 {
    let default_ctor = default_array_constructor(caller);
    if !value::is_object(exemplar) {
        return default_ctor;
    }
    let Some(exemplar_ptr) = resolve_handle(caller, exemplar) else {
        return default_ctor;
    };
    let mut constructor = read_object_property_by_name(caller, exemplar_ptr, "constructor")
        .unwrap_or_else(value::encode_undefined);
    if value::is_object(constructor) {
        if let Some(ctor_ptr) = resolve_handle(caller, constructor) {
            let species = read_object_property_by_name_id(
                caller,
                ctor_ptr,
                encode_symbol_name_id(WELL_KNOWN_SYMBOL_SPECIES),
            )
            .unwrap_or_else(value::encode_undefined);
            if value::is_null(species) {
                constructor = value::encode_undefined();
            } else if !value::is_undefined(species) {
                constructor = species;
            }
        }
    }
    if value::is_undefined(constructor) {
        default_ctor
    } else {
        constructor
    }
}

fn construct_array_with_constructor_sync(
    caller: &mut Caller<'_, RuntimeState>,
    constructor: i64,
    length: u32,
) -> i64 {
    // 仅原生 Array 构造器走快速路径；自定义构造器降级为 Array（避免同步 WASM 重入）
    if is_native_array_constructor(caller, constructor) || value::is_native_callable(constructor) {
        let len_val = value::encode_f64(length as f64);
        return call_native_callable_with_args_from_caller(
            caller,
            constructor,
            value::encode_undefined(),
            vec![len_val],
        )
        .unwrap_or_else(|| alloc_array(caller, length));
    }
    // 降级：用户自定义构造器不可安全同步调用 → 回退到普通 Array
    alloc_array(caller, length)
}

/// ES2024 `ArraySpeciesCreate(O, length)` — sync host paths (concat, slice, flat, splice).
pub(crate) fn array_species_create(
    caller: &mut Caller<'_, RuntimeState>,
    exemplar: i64,
    length: u32,
) -> i64 {
    let constructor = array_species_constructor(caller, exemplar);
    construct_array_with_constructor_sync(caller, constructor, length)
}

/// ES2024 `ArraySpeciesCreate(O, length)` — async host paths (map, filter, flatMap).
pub(crate) async fn array_species_create_async(
    caller: &mut Caller<'_, RuntimeState>,
    exemplar: i64,
    length: u32,
) -> i64 {
    let constructor = array_species_constructor(caller, exemplar);
    if is_native_array_constructor(caller, constructor) {
        return alloc_array(caller, length);
    }
    if !is_constructor_in_runtime(caller, constructor) {
        return value::encode_undefined();
    }
    let env = WasmEnv::from_caller(caller).expect("WasmEnv in array_species_create_async");
    let len_val = value::encode_f64(length as f64);
    let Some(shadow_sp) =
        crate::runtime_host_helpers::push_args_to_shadow_stack(caller, &env, &[len_val])
    else {
        return value::encode_undefined();
    };
    let previous_new_target = caller
        .data()
        .new_target
        .swap(constructor, Ordering::Relaxed);
    let result =
        resolve_and_call_async(caller, constructor, value::encode_undefined(), shadow_sp, 1).await;
    crate::runtime_host_helpers::restore_shadow_sp(caller, &env, shadow_sp);
    caller
        .data()
        .new_target
        .store(previous_new_target, Ordering::Relaxed);
    if value::is_object(result) {
        result
    } else {
        value::encode_undefined()
    }
}

pub(crate) fn alloc_array_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    let size = constants::HEAP_OBJECT_HEADER_SIZE
        .saturating_add(capacity.saturating_mul(constants::HEAP_ARRAY_ELEMENT_SIZE));
    let Some(ptr) =
        alloc_heap_region_for_host(ctx, env, size as usize, wjsm_ir::HEAP_TYPE_ARRAY, capacity)
    else {
        return value::encode_undefined();
    };
    let heap_ptr = ptr as u32;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let proto = env.array_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);
    if !host_handle_slot_fits(env, ctx, obj_table_count) {
        return value::encode_undefined();
    }
    let ptr = heap_ptr as usize;
    let slot_addr = obj_table_ptr as usize
        + obj_table_count as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize;
    let d = env.memory.data_mut(&mut *ctx);
    d[ptr + constants::HEAP_OBJECT_PROTO_OFFSET as usize
        ..ptr + constants::HEAP_OBJECT_PROTO_OFFSET as usize + 4]
        .copy_from_slice(&proto.to_le_bytes());
    d[ptr + constants::HEAP_OBJECT_TYPE_OFFSET as usize] = wjsm_ir::HEAP_TYPE_ARRAY;
    d[ptr + constants::HEAP_OBJECT_HEADER_PAD_START as usize
        ..ptr + constants::HEAP_OBJECT_HEADER_PAD_END as usize]
        .fill(0);
    d[ptr + constants::HEAP_ARRAY_LENGTH_OFFSET as usize
        ..ptr + constants::HEAP_ARRAY_LENGTH_OFFSET as usize + 4]
        .copy_from_slice(&0u32.to_le_bytes());
    d[ptr + constants::HEAP_ARRAY_CAPACITY_OFFSET as usize
        ..ptr + constants::HEAP_ARRAY_CAPACITY_OFFSET as usize + 4]
        .copy_from_slice(&capacity.to_le_bytes());
    d[slot_addr..slot_addr + constants::HANDLE_TABLE_ENTRY_SIZE as usize]
        .copy_from_slice(&heap_ptr.to_le_bytes());
    let _ = d;
    let _ = env
        .obj_table_count
        .set(&mut *ctx, Val::I32((obj_table_count + 1) as i32));
    value::encode_handle(value::TAG_ARRAY, obj_table_count)
}
// ── arr_proto_push (#49) ──────────────────────────────────────────
/// 从 host 元素设置数组元素（直接写入堆内存）
pub(crate) fn set_array_elem_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    arr_val: i64,
    index: i32,
    val: i64,
) {
    if !value::is_array(arr_val) {
        return;
    }
    let handle = value::decode_handle(arr_val) as usize;
    let Some(ptr) = resolve_handle_idx_with_env(ctx, env, handle) else {
        return;
    };
    let data = env.memory.data_mut(&mut *ctx);
    let slot_offset = ptr
        + constants::HEAP_OBJECT_HEADER_SIZE as usize
        + index as usize * constants::HEAP_ARRAY_ELEMENT_SIZE as usize;
    if slot_offset + constants::HEAP_ARRAY_ELEMENT_SIZE as usize > data.len() {
        return;
    }
    data[slot_offset..slot_offset + constants::HEAP_ARRAY_ELEMENT_SIZE as usize]
        .copy_from_slice(&val.to_le_bytes());
    // Update length to max(length, index+1)
    let length_offset = ptr + constants::HEAP_ARRAY_LENGTH_OFFSET as usize;
    let old_len = u32::from_le_bytes(
        data[length_offset..length_offset + 4]
            .try_into()
            .expect("array length"),
    );
    if (index as u32) >= old_len {
        let new_len = (index as u32) + 1;
        data[length_offset..length_offset + 4].copy_from_slice(&new_len.to_le_bytes());
    }
}
// ── 辅助函数：分配新对象 ────────────────────────────────────────────
pub(crate) fn alloc_object_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    let size = constants::HEAP_OBJECT_HEADER_SIZE
        .saturating_add(capacity.saturating_mul(constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE));
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    if !host_handle_slot_fits(env, ctx, obj_table_count) {
        return value::encode_undefined();
    }
    let Some(ptr) =
        alloc_heap_region_for_host(ctx, env, size as usize, wjsm_ir::HEAP_TYPE_OBJECT, capacity)
    else {
        return value::encode_undefined();
    };
    let heap_ptr = ptr as u32;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let ptr = heap_ptr as usize;
    let slot_addr = obj_table_ptr as usize
        + obj_table_count as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize;
    {
        let d = env.memory.data_mut(&mut *ctx);
        d[ptr + constants::HEAP_OBJECT_PROTO_OFFSET as usize
            ..ptr + constants::HEAP_OBJECT_PROTO_OFFSET as usize + 4]
            .copy_from_slice(&0u32.to_le_bytes()); // proto = 0 (null)
        d[ptr + constants::HEAP_OBJECT_TYPE_OFFSET as usize] = wjsm_ir::HEAP_TYPE_OBJECT;
        d[ptr + constants::HEAP_OBJECT_HEADER_PAD_START as usize
            ..ptr + constants::HEAP_OBJECT_HEADER_PAD_END as usize]
            .fill(0);
        d[ptr + constants::HEAP_OBJECT_CAPACITY_OFFSET as usize
            ..ptr + constants::HEAP_OBJECT_CAPACITY_OFFSET as usize + 4]
            .copy_from_slice(&capacity.to_le_bytes()); // capacity
        d[ptr + constants::HEAP_OBJECT_PROPERTY_COUNT_OFFSET as usize
            ..ptr + constants::HEAP_OBJECT_PROPERTY_COUNT_OFFSET as usize + 4]
            .copy_from_slice(&0u32.to_le_bytes()); // num_props = 0
        d[slot_addr..slot_addr + constants::HANDLE_TABLE_ENTRY_SIZE as usize]
            .copy_from_slice(&heap_ptr.to_le_bytes());
    }
    let _ = env
        .obj_table_count
        .set(&mut *ctx, Val::I32((obj_table_count + 1) as i32));
    value::encode_object_handle(obj_table_count)
}

/// 查找 name 对应的 nul 结尾 c-string 在线性内存中的偏移（即 name_id）。
///
/// 性能不变量：**只扫描 [0, heap_ptr) 的已分配区间，且用 SIMD 子串搜索（memchr::memmem）。**
/// 线性内存默认 256KB，但 bootstrap 期实际数据只占开头 ~80KB（heap_ptr 是 bump 分配上界）；
/// 大量 builtin 属性名注定找不到（miss），若朴素 windows() 全扫整块 256KB 确认不存在，
/// 会逐字节比掉 ~70% 的空尾部 —— 这曾是空程序执行开销的最大头（~70% 指令）。
/// 新增按名查找属性的代码请复用本函数，切勿另写裸的全内存 windows()/逐字节扫描。
pub(crate) fn find_memory_c_string_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    name: &str,
) -> Option<u32> {
    let mut needle = Vec::with_capacity(name.len() + 1);
    needle.extend_from_slice(name.as_bytes());
    needle.push(0);
    let heap_end = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as usize;
    let data = env.memory.data(&*ctx);
    let scan_end = heap_end.min(data.len());
    // 必须匹配完整的 nul 结尾 c-string，而非任意子串。data section 中字符串
    // 紧凑排布、每个串前一字节是上一个串的 nul 终止符，因此合法起点必满足
    // `offset == 0 || data[offset-1] == 0`。否则形如 "Array" 会错误匹配进
    // "isArray" 内部（offset+2），导致 name_id 与编译期 intern 偏移不一致。
    memchr::memmem::find_iter(&data[..scan_end], &needle)
        .find(|&offset| offset == 0 || data[offset - 1] == 0)
        .map(|offset| offset as u32)
}

pub(crate) fn alloc_heap_c_string_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    name: &str,
) -> Option<u32> {
    let bytes = name.as_bytes();
    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as usize;
    let end = heap_ptr.checked_add(bytes.len() + 1)?;
    let aligned_end = (end + 7) & !7;
    if !crate::runtime_heap::ensure_heap_allocation_bytes(
        ctx,
        env,
        heap_ptr,
        aligned_end.saturating_sub(heap_ptr),
    ) {
        return None;
    }
    {
        let data = env.memory.data_mut(&mut *ctx);
        data[heap_ptr..heap_ptr + bytes.len()].copy_from_slice(bytes);
        data[heap_ptr + bytes.len()] = 0;
        data[end..aligned_end].fill(0);
    }
    let _ = env.heap_ptr.set(&mut *ctx, Val::I32(aligned_end as i32));
    Some(heap_ptr as u32)
}
