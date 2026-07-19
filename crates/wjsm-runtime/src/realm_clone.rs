//! 主 realm pristine 可达图克隆 → 新 realm handle 区。
//!
//! 禁止整段 immortal memcpy / 二次 snapshot restore；只对可达闭包中的对象逐个
//! 分配 dynamic 槽并复制后做 ObjectHandleMapPolicy 重映射。

#[cfg(not(feature = "managed-heap-v2"))]
use std::collections::{HashSet, VecDeque};
#[cfg(not(feature = "managed-heap-v2"))]
use std::sync::atomic::Ordering;

use anyhow::Result;
#[cfg(not(feature = "managed-heap-v2"))]
use anyhow::{Context, bail};
use wasmtime::AsContextMut;
#[cfg(not(feature = "managed-heap-v2"))]
use wjsm_ir::constants::{
    FLAG_IS_ACCESSOR, HANDLE_TABLE_ENTRY_SIZE, HEAP_ARRAY_CAPACITY_OFFSET, HEAP_ARRAY_ELEMENT_SIZE,
    HEAP_OBJECT_CAPACITY_OFFSET, HEAP_OBJECT_HEADER_SIZE, HEAP_OBJECT_PROPERTY_SLOT_SIZE,
    HEAP_OBJECT_PROTO_OFFSET, HEAP_OBJECT_TYPE_OFFSET, PROP_SLOT_FLAGS_OFFSET,
    PROP_SLOT_GETTER_OFFSET, PROP_SLOT_SETTER_OFFSET, PROP_SLOT_SIZE, PROP_SLOT_VALUE_OFFSET,
};
use wjsm_ir::value;
#[cfg(not(feature = "managed-heap-v2"))]
use wjsm_ir::{HEAP_TYPE_ARRAY, HEAP_TYPE_OBJECT};

#[cfg(not(feature = "managed-heap-v2"))]
use crate::RuntimeOptions;
use crate::RuntimeState;
#[cfg(not(feature = "managed-heap-v2"))]
use crate::compile_source;
#[cfg(not(feature = "managed-heap-v2"))]
use crate::handle_remap::{HandleMap, ObjectHandleMapPolicy, RemapPolicy, remap_object_at};
use crate::realm::{Realm, RealmIntrinsics, main_realm_intrinsics_from_state};
#[cfg(not(feature = "managed-heap-v2"))]
use crate::realm::{RealmId, TYPEDARRAY_PROTO_COUNT, max_realms_limit};
#[cfg(not(feature = "managed-heap-v2"))]
use crate::runtime_gc::object_walker::resolve_handle;
#[cfg(not(feature = "managed-heap-v2"))]
use crate::runtime_heap::{
    alloc_heap_region_for_host, alloc_host_object, host_handle_slot_fits, set_object_proto_header,
};
#[cfg(not(feature = "managed-heap-v2"))]
use crate::runtime_startup::{
    compile_or_load_cached, instantiate_execute_bundle, run_startup_cold_path,
    startup_engine_config,
};
use crate::wasm_env::WasmEnv;

/// 从主 realm 的 WASM global + RuntimeState 字段装配 intrinsics。
pub(crate) fn main_realm_intrinsics_from_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
) -> RealmIntrinsics {
    let object_proto = {
        let h = env.object_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);
        if h < 0 {
            value::encode_undefined()
        } else {
            value::encode_object_handle(h as u32)
        }
    };
    let array_proto = {
        let h = env.array_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);
        if h < 0 {
            value::encode_undefined()
        } else {
            value::encode_object_handle(h as u32)
        }
    };
    let st = ctx.as_context().data();
    main_realm_intrinsics_from_state(
        object_proto,
        array_proto,
        st.iterator_prototype,
        st.generator_prototype,
        st.async_iterator_prototype,
        st.async_gen_prototype,
        st.symbol_prototype,
        st.promise_prototype,
        st.function_prototype,
        st.regexp_prototype,
        st.date_prototype,
        st.buffer_prototype,
        st.text_encoder_prototype,
        st.text_decoder_prototype,
        st.error_prototypes,
        st.typedarray_prototypes,
    )
}

/// 惰性登记主 realm 为 `active_realms[0]`。
#[cfg(not(feature = "managed-heap-v2"))]
pub(crate) fn main_realm_lazy_register<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
) {
    let needs = {
        let st = ctx.as_context().data();
        let realms = st.active_realms.lock().unwrap_or_else(|e| e.into_inner());
        realms.is_empty()
    };
    if !needs {
        return;
    }
    let intrinsics = main_realm_intrinsics_from_env(ctx, env);
    let global = ctx
        .as_context()
        .data()
        .js_global_object
        .load(Ordering::Relaxed);
    let realm = Realm::new(RealmId(0), global, intrinsics);
    let mut realms = ctx
        .as_context()
        .data()
        .active_realms
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if realms.is_empty() {
        realms.push(realm);
    }
}

#[cfg(not(feature = "managed-heap-v2"))]
/// 从 intrinsic 根 BFS 收集 obj_table handle 闭包（仅 object/array 堆对象）。
pub(crate) fn primordial_reachable_closure<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    roots: &RealmIntrinsics,
) -> Result<Vec<u32>> {
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;

    let mut seen: HashSet<u32> = HashSet::new();
    let mut queue: VecDeque<u32> = VecDeque::new();

    for raw in roots.iter_roots() {
        push_obj_handle(raw, obj_table_count, &mut seen, &mut queue);
    }

    while let Some(h) = queue.pop_front() {
        let children = {
            let data = env.memory.data(&*ctx);
            collect_object_child_handles(data, h, obj_table_ptr, obj_table_count)
        };
        for child in children {
            if seen.insert(child) {
                queue.push_back(child);
            }
        }
    }

    Ok(seen.into_iter().collect())
}

#[cfg(not(feature = "managed-heap-v2"))]
fn push_obj_handle(
    raw: i64,
    obj_table_count: usize,
    seen: &mut HashSet<u32>,
    queue: &mut VecDeque<u32>,
) {
    if !(value::is_object(raw) || value::is_array(raw)) {
        return;
    }
    let h = value::decode_object_handle(raw);
    if (h as usize) >= obj_table_count {
        return;
    }
    if seen.insert(h) {
        queue.push_back(h);
    }
}

#[cfg(not(feature = "managed-heap-v2"))]
fn collect_object_child_handles(
    data: &[u8],
    h: u32,
    obj_table_ptr: usize,
    obj_table_count: usize,
) -> Vec<u32> {
    let mut out = Vec::new();
    let Some(ptr) = resolve_handle(data, h, obj_table_ptr, obj_table_count) else {
        return out;
    };
    if ptr + HEAP_OBJECT_HEADER_SIZE as usize > data.len() {
        return out;
    }
    // proto
    let proto = u32::from_le_bytes(data[ptr..ptr + 4].try_into().unwrap_or([0; 4]));
    if proto != u32::MAX && (proto as usize) < obj_table_count {
        out.push(proto);
    }

    let heap_type = data[ptr + HEAP_OBJECT_TYPE_OFFSET as usize];
    if heap_type == HEAP_TYPE_OBJECT {
        let capacity = u32::from_le_bytes(
            data[ptr + HEAP_OBJECT_CAPACITY_OFFSET as usize
                ..ptr + HEAP_OBJECT_CAPACITY_OFFSET as usize + 4]
                .try_into()
                .unwrap_or([0; 4]),
        );
        let props = ptr + HEAP_OBJECT_HEADER_SIZE as usize;
        for slot in 0..capacity as usize {
            let slot_off = props + slot * PROP_SLOT_SIZE as usize;
            if slot_off + PROP_SLOT_SIZE as usize > data.len() {
                break;
            }
            let flags = i32::from_le_bytes(
                data[slot_off + PROP_SLOT_FLAGS_OFFSET as usize
                    ..slot_off + PROP_SLOT_FLAGS_OFFSET as usize + 4]
                    .try_into()
                    .unwrap_or([0; 4]),
            );
            if flags & FLAG_IS_ACCESSOR != 0 {
                push_raw_handle(
                    read_i64(data, slot_off + PROP_SLOT_GETTER_OFFSET as usize),
                    obj_table_count,
                    &mut out,
                );
                push_raw_handle(
                    read_i64(data, slot_off + PROP_SLOT_SETTER_OFFSET as usize),
                    obj_table_count,
                    &mut out,
                );
            } else {
                push_raw_handle(
                    read_i64(data, slot_off + PROP_SLOT_VALUE_OFFSET as usize),
                    obj_table_count,
                    &mut out,
                );
            }
        }
    } else if heap_type == HEAP_TYPE_ARRAY {
        let capacity = u32::from_le_bytes(
            data[ptr + HEAP_ARRAY_CAPACITY_OFFSET as usize
                ..ptr + HEAP_ARRAY_CAPACITY_OFFSET as usize + 4]
                .try_into()
                .unwrap_or([0; 4]),
        );
        let elems = ptr + HEAP_OBJECT_HEADER_SIZE as usize;
        for i in 0..capacity as usize {
            let off = elems + i * HEAP_ARRAY_ELEMENT_SIZE as usize;
            if off + 8 > data.len() {
                break;
            }
            push_raw_handle(read_i64(data, off), obj_table_count, &mut out);
        }
    }
    out
}

#[cfg(not(feature = "managed-heap-v2"))]
fn read_i64(data: &[u8], off: usize) -> i64 {
    if off + 8 > data.len() {
        return value::encode_undefined();
    }
    i64::from_le_bytes(data[off..off + 8].try_into().unwrap_or([0; 8]))
}

#[cfg(not(feature = "managed-heap-v2"))]
fn push_raw_handle(raw: i64, obj_table_count: usize, out: &mut Vec<u32>) {
    if value::is_object(raw) || value::is_array(raw) {
        let h = value::decode_object_handle(raw);
        if (h as usize) < obj_table_count {
            out.push(h);
        }
    }
}

#[cfg(feature = "managed-heap-v2")]
pub(crate) fn clone_pristine_realm<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    sandbox_global: i64,
) -> Result<Realm> {
    crate::realm_clone_v2::clone_pristine_realm_v2(ctx, env, sandbox_global)
}

#[cfg(not(feature = "managed-heap-v2"))]
/// 克隆 pristine 图到新 handle，返回已登记的 Realm。
pub(crate) fn clone_pristine_realm<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    sandbox_global: i64,
) -> Result<Realm> {
    main_realm_lazy_register(ctx, env);

    {
        let limit = max_realms_limit() as usize;
        let n = ctx
            .as_context()
            .data()
            .active_realms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        if n >= limit {
            bail!("vm: WJSM_VM_MAX_REALMS limit reached ({limit})");
        }
    }

    let roots = main_realm_intrinsics_from_env(ctx, env);
    let mut handles = primordial_reachable_closure(ctx, env, &roots)?;
    if handles.is_empty() {
        bail!("vm: main realm primordial graph is empty (bootstrap incomplete?)");
    }
    handles.sort_unstable();

    let mut map = HandleMap::new();
    for &old_h in &handles {
        let new_h = duplicate_heap_object(ctx, env, old_h)
            .with_context(|| format!("clone handle {old_h}"))?;
        map.insert(old_h, new_h);
    }

    remap_cloned_objects(ctx, env, &map)?;

    let remapped_intrinsics = remap_intrinsics(&roots, &map);
    let realm_id = {
        let next = ctx
            .as_context()
            .data()
            .next_realm_id
            .fetch_add(1, Ordering::Relaxed);
        RealmId(next)
    };

    if (value::is_object(sandbox_global) || value::is_array(sandbox_global))
        && value::is_object(remapped_intrinsics.object_proto)
    {
        set_object_proto_header(ctx, env, sandbox_global, remapped_intrinsics.object_proto);
    }

    let realm = Realm::new(realm_id, sandbox_global, remapped_intrinsics);
    {
        let mut realms = ctx
            .as_context()
            .data()
            .active_realms
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        realms.push(realm.clone());
    }
    Ok(realm)
}

#[cfg(not(feature = "managed-heap-v2"))]
fn remap_intrinsics(roots: &RealmIntrinsics, map: &HandleMap) -> RealmIntrinsics {
    let map_val = |raw: i64| -> i64 {
        if value::is_object(raw) {
            let h = value::decode_object_handle(raw);
            if let Some(n) = map.get(h) {
                return value::encode_object_handle(n);
            }
        }
        if value::is_array(raw) {
            let h = value::decode_array_handle(raw);
            if let Some(n) = map.get(h) {
                return value::encode_handle(value::TAG_ARRAY, n);
            }
        }
        raw
    };
    let mut ta = [RealmIntrinsics::UNDEFINED; TYPEDARRAY_PROTO_COUNT];
    for (i, v) in roots.typedarray_prototypes.iter().enumerate() {
        ta[i] = map_val(*v);
    }
    RealmIntrinsics {
        object_proto: map_val(roots.object_proto),
        array_proto: map_val(roots.array_proto),
        function_proto: map_val(roots.function_proto),
        iterator_prototype: map_val(roots.iterator_prototype),
        generator_prototype: map_val(roots.generator_prototype),
        async_iterator_prototype: map_val(roots.async_iterator_prototype),
        async_gen_prototype: map_val(roots.async_gen_prototype),
        symbol_prototype: map_val(roots.symbol_prototype),
        promise_prototype: map_val(roots.promise_prototype),
        regexp_prototype: map_val(roots.regexp_prototype),
        date_prototype: map_val(roots.date_prototype),
        error_proto: map_val(roots.error_proto),
        type_error_proto: map_val(roots.type_error_proto),
        range_error_proto: map_val(roots.range_error_proto),
        reference_error_proto: map_val(roots.reference_error_proto),
        syntax_error_proto: map_val(roots.syntax_error_proto),
        eval_error_proto: map_val(roots.eval_error_proto),
        uri_error_proto: map_val(roots.uri_error_proto),
        aggregate_error_proto: map_val(roots.aggregate_error_proto),
        buffer_prototype: map_val(roots.buffer_prototype),
        text_encoder_prototype: map_val(roots.text_encoder_prototype),
        text_decoder_prototype: map_val(roots.text_decoder_prototype),
        typedarray_prototypes: ta,
    }
}

#[cfg(not(feature = "managed-heap-v2"))]
fn duplicate_heap_object<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    old_h: u32,
) -> Result<u32> {
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0).max(0) as u32;

    let (heap_type, capacity, src_bytes) = {
        let data = env.memory.data(&*ctx);
        let Some(ptr) = resolve_handle(data, old_h, obj_table_ptr, obj_table_count as usize) else {
            bail!("invalid source handle {old_h}");
        };
        let heap_type = data[ptr + HEAP_OBJECT_TYPE_OFFSET as usize];
        let (cap_off, elem_size) = if heap_type == HEAP_TYPE_ARRAY {
            (HEAP_ARRAY_CAPACITY_OFFSET, HEAP_ARRAY_ELEMENT_SIZE)
        } else if heap_type == HEAP_TYPE_OBJECT {
            (HEAP_OBJECT_CAPACITY_OFFSET, HEAP_OBJECT_PROPERTY_SLOT_SIZE)
        } else {
            bail!("unsupported heap type {heap_type} for handle {old_h}");
        };
        let capacity = u32::from_le_bytes(
            data[ptr + cap_off as usize..ptr + cap_off as usize + 4]
                .try_into()
                .context("capacity")?,
        );
        let size = HEAP_OBJECT_HEADER_SIZE as usize + capacity as usize * elem_size as usize;
        if ptr + size > data.len() {
            bail!("source object extends past memory");
        }
        (heap_type, capacity, data[ptr..ptr + size].to_vec())
    };

    let size = src_bytes.len();
    let Some(new_ptr) = alloc_heap_region_for_host(ctx, env, size, heap_type, capacity) else {
        bail!("OOM allocating clone of handle {old_h}");
    };
    let new_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    if !host_handle_slot_fits(env, ctx, new_count) {
        bail!("handle table full while cloning");
    }
    let table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;
    let slot_addr = table_ptr + new_count as usize * HANDLE_TABLE_ENTRY_SIZE as usize;
    {
        let data = env.memory.data_mut(&mut *ctx);
        data[new_ptr..new_ptr + size].copy_from_slice(&src_bytes);
        data[slot_addr..slot_addr + HANDLE_TABLE_ENTRY_SIZE as usize]
            .copy_from_slice(&(new_ptr as u32).to_le_bytes());
    }
    let _ = env
        .obj_table_count
        .set(&mut *ctx, wasmtime::Val::I32((new_count + 1) as i32));
    Ok(new_count)
}

#[cfg(not(feature = "managed-heap-v2"))]
fn remap_cloned_objects<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    map: &HandleMap,
) -> Result<()> {
    let policy = ObjectHandleMapPolicy { map };
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;

    for new_h in map.values() {
        // 需要可写 memory；每次循环重新 borrow
        let data = env.memory.data_mut(&mut *ctx);
        let Some(ptr) = resolve_handle(data, new_h, obj_table_ptr, obj_table_count) else {
            continue;
        };
        let heap_type = data[ptr + HEAP_OBJECT_TYPE_OFFSET as usize];
        if heap_type == HEAP_TYPE_OBJECT {
            let capacity = u32::from_le_bytes(
                data[ptr + HEAP_OBJECT_CAPACITY_OFFSET as usize
                    ..ptr + HEAP_OBJECT_CAPACITY_OFFSET as usize + 4]
                    .try_into()
                    .context("cap")?,
            );
            remap_object_at(data, ptr, capacity, &policy)?;
        } else if heap_type == HEAP_TYPE_ARRAY {
            let capacity = u32::from_le_bytes(
                data[ptr + HEAP_ARRAY_CAPACITY_OFFSET as usize
                    ..ptr + HEAP_ARRAY_CAPACITY_OFFSET as usize + 4]
                    .try_into()
                    .context("arr cap")?,
            );
            let proto_off = ptr + HEAP_OBJECT_PROTO_OFFSET as usize;
            if proto_off + 4 <= data.len() {
                let old = u32::from_le_bytes(data[proto_off..proto_off + 4].try_into().unwrap());
                let new_p = policy.remap_proto_handle(old);
                data[proto_off..proto_off + 4].copy_from_slice(&new_p.to_le_bytes());
            }
            let elems = ptr + HEAP_OBJECT_HEADER_SIZE as usize;
            for i in 0..capacity as usize {
                let off = elems + i * HEAP_ARRAY_ELEMENT_SIZE as usize;
                if off + 8 > data.len() {
                    break;
                }
                let raw = i64::from_le_bytes(data[off..off + 8].try_into().unwrap());
                let remapped = policy.remap_value(raw);
                if remapped != raw {
                    data[off..off + 8].copy_from_slice(&remapped.to_le_bytes());
                }
            }
        }
    }
    Ok(())
}

/// 测试探针结果。
#[derive(Debug, Clone)]
pub struct RealmCloneProbe {
    pub main_array_proto_handle: u32,
    pub clone_array_proto_handle: u32,
    pub main_object_proto_handle: u32,
    pub clone_object_proto_handle: u32,
    /// clone.array_proto 的 [[Prototype]] handle
    pub clone_array_proto_of: u32,
    pub realm_id: u32,
    pub closure_size: usize,
    /// 闭包内每个对象的子 handle 均在闭包内（无悬挂堆引用）
    pub closure_closed: bool,
    /// 全部 RealmIntrinsics 根（有效 object/array）均落在闭包内
    pub roots_covered: bool,
}

/// 执行帧探针：enter 克隆 realm 后 WASM global 是否切到新 array/object proto，exit 是否恢复。
#[derive(Debug, Clone)]
pub struct ExecutionRealmFrameProbe {
    pub main_array: i32,
    pub main_object: i32,
    pub inside_array: i32,
    pub inside_object: i32,
    pub after_array: i32,
    pub after_object: i32,
    pub inside_execution_realm: u32,
    pub after_execution_realm: u32,
}

#[cfg(not(feature = "managed-heap-v2"))]
/// Bootstrap + clone 后验证 `with_execution_realm_frame` 的 global swap。
pub async fn probe_execution_realm_frame() -> Result<ExecutionRealmFrameProbe> {
    use crate::realm::{RealmId, with_execution_realm_frame};

    let wasm = compile_source("/* execution realm frame probe */")?;
    let engine = startup_engine_config(true, None, false)
        .build()
        .map_err(|e| anyhow::anyhow!("failed to create engine: {e:?}"))?;
    let module = compile_or_load_cached(&engine, &wasm)?;
    let mut bundle =
        instantiate_execute_bundle(&engine, &module, None, true, RuntimeOptions::default()).await?;
    run_startup_cold_path(&mut bundle).await?;

    let env = bundle.wasm_env;
    let store = &mut bundle.store;

    let sandbox = alloc_host_object(store, &env, 4);
    if !value::is_object(sandbox) {
        bail!("sandbox alloc failed");
    }
    let realm = clone_pristine_realm(store, &env, sandbox)?;

    let main_array = env.array_proto_handle.get(&mut *store).i32().unwrap_or(-1);
    let main_object = env.object_proto_handle.get(&mut *store).i32().unwrap_or(-1);

    let mut inside_array = -1;
    let mut inside_object = -1;
    let mut inside_execution_realm = 0u32;

    with_execution_realm_frame(store, &env, realm.id, |store| {
        inside_array = env.array_proto_handle.get(&mut *store).i32().unwrap_or(-1);
        inside_object = env.object_proto_handle.get(&mut *store).i32().unwrap_or(-1);
        inside_execution_realm = store.data().execution_realm.load(Ordering::Relaxed);
    });

    let after_array = env.array_proto_handle.get(&mut *store).i32().unwrap_or(-1);
    let after_object = env.object_proto_handle.get(&mut *store).i32().unwrap_or(-1);
    let after_execution_realm = store.data().execution_realm.load(Ordering::Relaxed);

    let _ = RealmId(0); // 保留类型可见性

    Ok(ExecutionRealmFrameProbe {
        main_array,
        main_object,
        inside_array,
        inside_object,
        after_array,
        after_object,
        inside_execution_realm,
        after_execution_realm,
    })
}

/// 在克隆 realm 执行帧内分配 `[]` 同源数组，对照 proto handle。
///
/// 解释器 `eval_array_lit` 与 compiled `arr_new` / `ArrayConstructor` 均经
/// `alloc_array_with_env` 读 `__array_proto_handle`；帧 swap 后三者同源。
#[derive(Debug, Clone)]
pub struct EvalRealmArrayProbe {
    pub realm_array_proto: u32,
    pub result_proto: u32,
    pub main_array_proto: u32,
}

#[cfg(not(feature = "managed-heap-v2"))]
/// Task 2.1/2.2：执行帧内数组分配 [[Prototype]] === realm.array_proto。
pub async fn probe_eval_array_literal_in_realm() -> Result<EvalRealmArrayProbe> {
    use crate::realm::with_execution_realm_frame;

    let wasm = compile_source("/* eval realm array probe */")?;
    let engine = startup_engine_config(true, None, false)
        .build()
        .map_err(|e| anyhow::anyhow!("failed to create engine: {e:?}"))?;
    let module = compile_or_load_cached(&engine, &wasm)?;
    let mut bundle =
        instantiate_execute_bundle(&engine, &module, None, true, RuntimeOptions::default()).await?;
    run_startup_cold_path(&mut bundle).await?;

    let env = bundle.wasm_env;
    let store = &mut bundle.store;

    let main_array_proto = env.array_proto_handle.get(&mut *store).i32().unwrap_or(-1) as u32;
    let sandbox = alloc_host_object(store, &env, 4);
    let realm = clone_pristine_realm(store, &env, sandbox)?;
    let realm_array_proto = value::decode_object_handle(realm.intrinsics.array_proto);
    let realm_id = realm.id;

    let mut result_proto = u32::MAX;
    with_execution_realm_frame(store, &env, realm_id, |store| {
        let arr = crate::runtime_host_helpers::alloc_array_with_env(store, &env, 1);
        if value::is_array(arr) {
            let h = value::decode_array_handle(arr);
            let obj_table_ptr =
                env.obj_table_ptr.get(&mut *store).i32().unwrap_or(0).max(0) as usize;
            let obj_table_count = env
                .obj_table_count
                .get(&mut *store)
                .i32()
                .unwrap_or(0)
                .max(0) as usize;
            let data = env.memory.data(&*store);
            if let Some(ptr) = resolve_handle(data, h, obj_table_ptr, obj_table_count) {
                result_proto = u32::from_le_bytes(data[ptr..ptr + 4].try_into().unwrap());
            }
        }
    });

    Ok(EvalRealmArrayProbe {
        realm_array_proto,
        result_proto,
        main_array_proto,
    })
}

/// Bootstrap 空模块、克隆 pristine realm，返回 handle 对照（集成测试入口）。
#[cfg(not(feature = "managed-heap-v2"))]
pub async fn probe_clone_pristine_realm() -> Result<RealmCloneProbe> {
    let wasm = compile_source("/* realm clone probe */")?;
    let engine = startup_engine_config(true, None, false)
        .build()
        .map_err(|e| anyhow::anyhow!("failed to create engine: {e:?}"))?;
    let module = compile_or_load_cached(&engine, &wasm)?;
    let mut bundle =
        instantiate_execute_bundle(&engine, &module, None, true, RuntimeOptions::default()).await?;
    run_startup_cold_path(&mut bundle).await?;

    let env = bundle.wasm_env;
    let store = &mut bundle.store;

    let main_array = env.array_proto_handle.get(&mut *store).i32().unwrap_or(-1);
    let main_object = env.object_proto_handle.get(&mut *store).i32().unwrap_or(-1);
    if main_array < 0 || main_object < 0 {
        bail!("bootstrap did not install array/object proto");
    }

    let roots = main_realm_intrinsics_from_env(store, &env);
    let closure = primordial_reachable_closure(store, &env, &roots)?;
    let closure_size = closure.len();
    let closure_set: HashSet<u32> = closure.iter().copied().collect();

    let obj_table_ptr = env.obj_table_ptr.get(&mut *store).i32().unwrap_or(0).max(0) as usize;
    let obj_table_count = env
        .obj_table_count
        .get(&mut *store)
        .i32()
        .unwrap_or(0)
        .max(0) as usize;

    let roots_covered = roots.iter_roots().all(|raw| {
        if value::is_object(raw) || value::is_array(raw) {
            let h = value::decode_object_handle(raw);
            (h as usize) >= obj_table_count || closure_set.contains(&h)
        } else {
            true // undefined 等非堆根跳过
        }
    });

    let closure_closed = {
        let data = env.memory.data(&*store);
        closure.iter().all(|&h| {
            collect_object_child_handles(data, h, obj_table_ptr, obj_table_count)
                .into_iter()
                .all(|c| closure_set.contains(&c))
        })
    };

    let sandbox = alloc_host_object(store, &env, 8);
    if !value::is_object(sandbox) {
        bail!("failed to alloc sandbox");
    }

    let realm = clone_pristine_realm(store, &env, sandbox)?;

    let clone_array = value::decode_object_handle(realm.intrinsics.array_proto);
    let clone_object = value::decode_object_handle(realm.intrinsics.object_proto);

    let clone_array_proto_of = {
        let obj_table_ptr = env.obj_table_ptr.get(&mut *store).i32().unwrap_or(0).max(0) as usize;
        let obj_table_count = env
            .obj_table_count
            .get(&mut *store)
            .i32()
            .unwrap_or(0)
            .max(0) as usize;
        let data = env.memory.data(&*store);
        let ptr = resolve_handle(data, clone_array, obj_table_ptr, obj_table_count)
            .context("resolve clone array_proto")?;
        u32::from_le_bytes(data[ptr..ptr + 4].try_into().unwrap())
    };

    Ok(RealmCloneProbe {
        main_array_proto_handle: main_array as u32,
        clone_array_proto_handle: clone_array,
        main_object_proto_handle: main_object as u32,
        clone_object_proto_handle: clone_object,
        clone_array_proto_of,
        realm_id: realm.id.0,
        closure_size,
        closure_closed,
        roots_covered,
    })
}
