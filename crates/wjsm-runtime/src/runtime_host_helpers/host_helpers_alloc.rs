use super::*;
pub(crate) fn alloc_array_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let size = 16u32.saturating_add(capacity.saturating_mul(8));
    let new_heap_ptr = heap_ptr.saturating_add(size);
    let proto = env.array_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);
    let d = env.memory.data_mut(&mut *ctx);
    let ptr = heap_ptr as usize;
    if (new_heap_ptr as usize) > d.len() {
        return value::encode_undefined();
    }
    let slot_addr = obj_table_ptr as usize + obj_table_count as usize * 4;
    // obj_table 槽位耗尽时必须直接返回 undefined，不递增 obj_table_count、
    // 不前进 heap_ptr，保持 handle->slot 映射与 obj_table_count 一致。
    if slot_addr + 4 > d.len() {
        return value::encode_undefined();
    }
    d[ptr..ptr + 4].copy_from_slice(&proto.to_le_bytes());
    d[ptr + 4] = 1u8;
    d[ptr + 5..ptr + 8].fill(0);
    d[ptr + 8..ptr + 12].copy_from_slice(&0u32.to_le_bytes());
    d[ptr + 12..ptr + 16].copy_from_slice(&capacity.to_le_bytes());
    d[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
    let _ = d;
    let _ = env.heap_ptr.set(&mut *ctx, Val::I32(new_heap_ptr as i32));
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
    let slot_offset = ptr + 16 + index as usize * 8;
    if slot_offset + 8 > data.len() {
        return;
    }
    data[slot_offset..slot_offset + 8].copy_from_slice(&val.to_le_bytes());
    // Update length to max(length, index+1)
    let old_len =
        u32::from_le_bytes([data[ptr + 8], data[ptr + 9], data[ptr + 10], data[ptr + 11]]);
    if (index as u32) >= old_len {
        let new_len = (index as u32) + 1;
        data[ptr + 8..ptr + 12].copy_from_slice(&new_len.to_le_bytes());
    }
}
// ── 辅助函数：分配新对象 ────────────────────────────────────────────
pub(crate) fn alloc_object_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let size = 16u32.saturating_add(capacity.saturating_mul(32));
    let new_heap_ptr = heap_ptr.saturating_add(size);
    let d = env.memory.data_mut(&mut *ctx);
    let ptr = heap_ptr as usize;
    if (new_heap_ptr as usize) > d.len() {
        return value::encode_undefined();
    }
    let slot_addr = obj_table_ptr as usize + obj_table_count as usize * 4;
    // obj_table 槽位耗尽时必须直接返回 undefined，不递增 obj_table_count、
    // 不前进 heap_ptr，保持 handle->slot 映射与 obj_table_count 一致。
    if slot_addr + 4 > d.len() {
        return value::encode_undefined();
    }
    d[ptr..ptr + 4].copy_from_slice(&0u32.to_le_bytes()); // proto = 0 (null)
    d[ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT;
    d[ptr + 5..ptr + 8].fill(0);
    d[ptr + 8..ptr + 12].copy_from_slice(&capacity.to_le_bytes()); // capacity
    d[ptr + 12..ptr + 16].copy_from_slice(&0u32.to_le_bytes()); // num_props = 0
    d[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
    let _ = d;
    let _ = env.heap_ptr.set(&mut *ctx, Val::I32(new_heap_ptr as i32));
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
    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0) as usize;
    let bytes = name.as_bytes();
    let end = heap_ptr.checked_add(bytes.len() + 1)?;
    let aligned_end = (end + 7) & !7;
    {
        let data = env.memory.data_mut(&mut *ctx);
        if aligned_end > data.len() {
            return None;
        }
        data[heap_ptr..heap_ptr + bytes.len()].copy_from_slice(bytes);
        data[heap_ptr + bytes.len()] = 0;
        data[end..aligned_end].fill(0);
    }
    let _ = env.heap_ptr.set(&mut *ctx, Val::I32(aligned_end as i32));
    Some(heap_ptr as u32)
}
