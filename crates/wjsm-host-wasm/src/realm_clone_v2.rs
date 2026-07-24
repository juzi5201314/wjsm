use std::collections::{HashSet, VecDeque};

use anyhow::{Context, Result, bail};
use wasmtime::AsContextMut;
use wjsm_ir::constants;
use wjsm_ir::value;

use crate::RuntimeState;
use crate::handle_remap::HandleMap;
use crate::heap::HandleTableV2;
use crate::realm::{Realm, RealmId, RealmIntrinsics};
use crate::runtime_gc::HeapAccessV2;
use crate::runtime_heap::alloc_host_object;
use crate::runtime_heap::allocate_v2_object_bytes_with_context;
use crate::runtime_startup::{
    compile_or_load_cached, instantiate_execute_bundle, run_startup_cold_path,
    startup_engine_config,
};
use crate::wasm_env::WasmEnv;
use crate::{RuntimeOptions, compile_source};

pub fn remap_realm_handles_v2(
    source: &Realm,
    id: RealmId,
    map: &HandleMap,
    handles: &HandleTableV2,
) -> Result<Realm> {
    remap_realm_values(source, id, map, |handle| {
        handles
            .resolve(crate::heap::HandleId::new(handle))
            .is_some()
    })
}

pub(crate) fn clone_pristine_realm_v2<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    sandbox_global: i64,
) -> Result<Realm> {
    let access = ctx.as_context().data().heap_access_v2().clone();
    let roots = super::realm_clone::main_realm_intrinsics_from_env(ctx, env);
    let handles = collect_reachable(&access, &roots)?;
    if handles.is_empty() {
        bail!("V2 primordial graph is empty (bootstrap incomplete?)");
    }

    let mut map = HandleMap::new();
    for &source in &handles {
        let target = duplicate_object(ctx, env, &access, source)
            .with_context(|| format!("clone V2 handle {source}"))?;
        map.insert(source, target);
    }
    for &source in &handles {
        remap_object(&access, &map, source, map.get(source).unwrap())?;
    }

    let realm_id = RealmId(
        ctx.as_context()
            .data()
            .next_realm_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    );
    let source_global = ctx
        .as_context()
        .data()
        .js_global_object
        .load(std::sync::atomic::Ordering::Relaxed);
    let source_realm = Realm::new(RealmId(0), source_global, roots);
    let realm = remap_realm_handles_v2_with_access(&source_realm, realm_id, &map, &access)?;
    if value::is_object(sandbox_global) || value::is_array(sandbox_global) {
        let sandbox = value::decode_handle(sandbox_global);
        let proto = value::decode_handle(realm.intrinsics.object_proto);
        access.set_prototype(sandbox, proto)?;
    }
    ctx.as_context()
        .data()
        .active_realms
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push(realm.clone());
    Ok(realm)
}

pub(crate) fn collect_reachable(
    access: &HeapAccessV2,
    roots: &RealmIntrinsics,
) -> Result<Vec<u32>> {
    let mut queue = VecDeque::new();
    let mut seen = HashSet::new();
    for raw in roots.iter_roots() {
        push_value(raw, &mut seen, &mut queue);
    }
    while let Some(handle) = queue.pop_front() {
        let prototype = access.prototype(handle)?;
        if prototype != u32::MAX && seen.insert(prototype) {
            queue.push_back(prototype);
        }
        match access.object_type(handle)? {
            ty if ty == u32::from(wjsm_ir::HEAP_TYPE_ARRAY) => {
                let (length, _) = access.array_shape(handle)?;
                for index in 0..length {
                    if let Some(value) = access.get_element(handle, index)? {
                        push_value(value as i64, &mut seen, &mut queue);
                    }
                }
            }
            ty if ty == u32::from(wjsm_ir::HEAP_TYPE_OBJECT) => {
                for (key, _) in access.own_property_slots(handle)? {
                    if let Some(property) = access.get_property_slot(handle, key)? {
                        push_value(property.value as i64, &mut seen, &mut queue);
                        push_value(property.getter as i64, &mut seen, &mut queue);
                        push_value(property.setter as i64, &mut seen, &mut queue);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(seen.into_iter().collect())
}

fn push_value(raw: i64, seen: &mut HashSet<u32>, queue: &mut VecDeque<u32>) {
    if value::is_object(raw) || value::is_array(raw) {
        let handle = value::decode_handle(raw);
        if seen.insert(handle) {
            queue.push_back(handle);
        }
    }
}

fn duplicate_object<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    access: &HeapAccessV2,
    source: u32,
) -> Result<u32> {
    let object_type = access.object_type(source)?;
    let (capacity, bytes) = if object_type == u32::from(wjsm_ir::HEAP_TYPE_ARRAY) {
        let (_, capacity) = access.array_shape(source)?;
        (
            capacity,
            u64::from(capacity) * u64::from(constants::HEAP_ARRAY_ELEMENT_SIZE)
                + u64::from(constants::HEAP_OBJECT_HEADER_SIZE),
        )
    } else {
        let slots = access.own_property_slots(source)?;
        let capacity = u32::try_from(slots.len().max(4))?;
        (
            capacity,
            u64::from(capacity) * u64::from(constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE)
                + u64::from(constants::HEAP_OBJECT_HEADER_SIZE),
        )
    };
    let handle = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0);
    env.obj_table_count
        .set(&mut *ctx, wasmtime::Val::I32(handle + 1))?;
    let (object, _) = allocate_v2_object_bytes_with_context(ctx, bytes)?;
    if object_type == u32::from(wjsm_ir::HEAP_TYPE_ARRAY) {
        access.publish_array(handle as u32, object, u32::MAX, capacity)?;
    } else {
        access.publish_object(handle as u32, object, u32::MAX, capacity)?;
    }
    Ok(handle as u32)
}

fn remap_object(access: &HeapAccessV2, map: &HandleMap, source: u32, target: u32) -> Result<()> {
    access.set_prototype(target, remap_handle(access.prototype(source)?, map)?)?;
    match access.object_type(source)? {
        ty if ty == u32::from(wjsm_ir::HEAP_TYPE_ARRAY) => {
            let (length, _) = access.array_shape(source)?;
            for index in 0..length {
                if let Some(raw) = access.get_element(source, index)? {
                    access.set_element(target, index, remap_value(raw as i64, map)?)?;
                }
            }
        }
        ty if ty == u32::from(wjsm_ir::HEAP_TYPE_OBJECT) => {
            for (key, _) in access.own_property_slots(source)? {
                let property = access
                    .get_property_slot(source, key)?
                    .context("V2 property disappeared during realm clone")?;
                if property.flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
                    access.define_accessor_property(
                        target,
                        key,
                        remap_value(property.getter as i64, map)?,
                        remap_value(property.setter as i64, map)?,
                    )?;
                } else {
                    access.define_data_property(
                        target,
                        key,
                        remap_value(property.value as i64, map)?,
                        property.flags,
                    )?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn remap_value(raw: i64, map: &HandleMap) -> Result<u64> {
    if !value::is_object(raw) && !value::is_array(raw) {
        return Ok(raw as u64);
    }
    let source = value::decode_handle(raw);
    let target = map
        .get(source)
        .ok_or_else(|| anyhow::anyhow!("V2 realm clone missing handle map entry {source}"))?;
    Ok(if value::is_object(raw) {
        value::encode_object_handle(target) as u64
    } else {
        value::encode_handle(value::TAG_ARRAY, target) as u64
    })
}

fn remap_handle(source: u32, map: &HandleMap) -> Result<u32> {
    if source == u32::MAX {
        return Ok(source);
    }
    map.get(source)
        .ok_or_else(|| anyhow::anyhow!("V2 realm clone missing prototype map entry {source}"))
}

fn remap_realm_values(
    source: &Realm,
    id: RealmId,
    map: &HandleMap,
    is_live: impl Fn(u32) -> bool,
) -> Result<Realm> {
    let remap = |raw: i64| -> Result<i64> {
        if !value::is_object(raw) && !value::is_array(raw) {
            return Ok(raw);
        }
        let source = crate::heap::HandleId::new(value::decode_handle(raw));
        if !is_live(source.get()) {
            bail!("V2 realm source handle {} is not live", source.get());
        }
        let target = map.remap_handle_v2(source);
        if !is_live(target.get()) {
            bail!("V2 realm target handle {} is not live", target.get());
        }
        Ok(if value::is_object(raw) {
            value::encode_object_handle(target.get())
        } else {
            value::encode_handle(value::TAG_ARRAY, target.get())
        })
    };
    let mut realm = Realm::new(
        id,
        remap(source.global_object)?,
        source.intrinsics.try_map_values(remap)?,
    );
    realm.code_generation = source.code_generation;
    realm.microtask_mode = source.microtask_mode;
    Ok(realm)
}

pub(crate) fn remap_realm_handles_v2_with_access(
    source: &Realm,
    id: RealmId,
    map: &HandleMap,
    access: &HeapAccessV2,
) -> Result<Realm> {
    remap_realm_values(source, id, map, |handle| {
        access.resolve_handle(handle).is_ok()
    })
}

async fn build_probe_bundle(source: &str) -> Result<crate::runtime_startup::ExecuteInstanceBundle> {
    let wasm = compile_source(source)?;
    let engine = startup_engine_config(true, None, false)
        .build()
        .map_err(|error| anyhow::anyhow!("failed to create V2 probe engine: {error:?}"))?;
    let module = compile_or_load_cached(&engine, &wasm)?;
    let mut bundle =
        instantiate_execute_bundle(&engine, &module, None, true, RuntimeOptions::default()).await?;
    run_startup_cold_path(&mut bundle).await?;
    Ok(bundle)
}

pub async fn probe_clone_pristine_realm_v2() -> Result<crate::realm_clone::RealmCloneProbe> {
    let mut bundle = build_probe_bundle("/* V2 realm clone probe */").await?;
    let env = bundle.wasm_env;
    let store = &mut bundle.store;
    let main_array = env.array_proto_handle.get(&mut *store).i32().unwrap_or(-1);
    let main_object = env.object_proto_handle.get(&mut *store).i32().unwrap_or(-1);
    if main_array < 0 || main_object < 0 {
        bail!("V2 bootstrap did not install array/object proto");
    }
    let roots = super::realm_clone::main_realm_intrinsics_from_env(store, &env);
    let access = store.data().heap_access_v2().clone();
    let closure = collect_reachable(&access, &roots)?;
    let closure_set = closure.iter().copied().collect::<HashSet<_>>();
    let roots_covered = roots.iter_roots().all(|raw| {
        (!value::is_object(raw) && !value::is_array(raw))
            || closure_set.contains(&value::decode_handle(raw))
    });
    let closure_closed = closure.iter().all(|handle| {
        object_child_handles(&access, *handle)
            .is_ok_and(|children| children.iter().all(|child| closure_set.contains(child)))
    });

    let sandbox = alloc_host_object(store, &env, 8);
    if !value::is_object(sandbox) {
        bail!("failed to allocate V2 sandbox");
    }
    let realm = clone_pristine_realm_v2(store, &env, sandbox)?;
    let clone_array = value::decode_handle(realm.intrinsics.array_proto);
    let clone_object = value::decode_handle(realm.intrinsics.object_proto);
    let clone_array_proto_of = access.prototype(clone_array)?;

    Ok(crate::realm_clone::RealmCloneProbe {
        main_array_proto_handle: main_array as u32,
        clone_array_proto_handle: clone_array,
        main_object_proto_handle: main_object as u32,
        clone_object_proto_handle: clone_object,
        clone_array_proto_of,
        realm_id: realm.id.0,
        closure_size: closure.len(),
        closure_closed,
        roots_covered,
    })
}

pub async fn probe_execution_realm_frame_v2() -> Result<crate::realm_clone::ExecutionRealmFrameProbe>
{
    use std::sync::atomic::Ordering;

    let mut bundle = build_probe_bundle("/* V2 execution realm frame probe */").await?;
    let env = bundle.wasm_env;
    let store = &mut bundle.store;
    let sandbox = alloc_host_object(store, &env, 4);
    let realm = clone_pristine_realm_v2(store, &env, sandbox)?;
    let main_array = env.array_proto_handle.get(&mut *store).i32().unwrap_or(-1);
    let main_object = env.object_proto_handle.get(&mut *store).i32().unwrap_or(-1);
    let mut inside_array = -1;
    let mut inside_object = -1;
    let mut inside_execution_realm = 0;
    crate::realm::with_execution_realm_frame(store, &env, realm.id, |store| {
        inside_array = env.array_proto_handle.get(&mut *store).i32().unwrap_or(-1);
        inside_object = env.object_proto_handle.get(&mut *store).i32().unwrap_or(-1);
        inside_execution_realm = store.data().execution_realm.load(Ordering::Relaxed);
    });
    Ok(crate::realm_clone::ExecutionRealmFrameProbe {
        main_array,
        main_object,
        inside_array,
        inside_object,
        after_array: env.array_proto_handle.get(&mut *store).i32().unwrap_or(-1),
        after_object: env.object_proto_handle.get(&mut *store).i32().unwrap_or(-1),
        inside_execution_realm,
        after_execution_realm: store.data().execution_realm.load(Ordering::Relaxed),
    })
}

pub async fn probe_eval_array_literal_in_realm_v2()
-> Result<crate::realm_clone::EvalRealmArrayProbe> {
    let mut bundle = build_probe_bundle("/* V2 eval realm array probe */").await?;
    let env = bundle.wasm_env;
    let store = &mut bundle.store;
    let main_array_proto = env.array_proto_handle.get(&mut *store).i32().unwrap_or(-1) as u32;
    let sandbox = alloc_host_object(store, &env, 4);
    let realm = clone_pristine_realm_v2(store, &env, sandbox)?;
    let realm_array_proto = value::decode_handle(realm.intrinsics.array_proto);
    let mut result_proto = u32::MAX;
    crate::realm::with_execution_realm_frame(store, &env, realm.id, |store| {
        let array = crate::runtime_host_helpers::alloc_array_with_env(store, &env, 1);
        if value::is_array(array) {
            result_proto = store
                .data()
                .heap_access_v2()
                .prototype(value::decode_handle(array))
                .unwrap_or(u32::MAX);
        }
    });
    Ok(crate::realm_clone::EvalRealmArrayProbe {
        realm_array_proto,
        result_proto,
        main_array_proto,
    })
}

fn object_child_handles(access: &HeapAccessV2, handle: u32) -> Result<Vec<u32>> {
    let mut children = Vec::new();
    let prototype = access.prototype(handle)?;
    if prototype != u32::MAX {
        children.push(prototype);
    }
    match access.object_type(handle)? {
        ty if ty == u32::from(wjsm_ir::HEAP_TYPE_ARRAY) => {
            let (length, _) = access.array_shape(handle)?;
            for index in 0..length {
                if let Some(raw) = access.get_element(handle, index)? {
                    append_value_handle(raw as i64, &mut children);
                }
            }
        }
        ty if ty == u32::from(wjsm_ir::HEAP_TYPE_OBJECT) => {
            for (key, _) in access.own_property_slots(handle)? {
                if let Some(property) = access.get_property_slot(handle, key)? {
                    append_value_handle(property.value as i64, &mut children);
                    append_value_handle(property.getter as i64, &mut children);
                    append_value_handle(property.setter as i64, &mut children);
                }
            }
        }
        _ => {}
    }
    Ok(children)
}

fn append_value_handle(raw: i64, handles: &mut Vec<u32>) {
    if value::is_object(raw) || value::is_array(raw) {
        handles.push(value::decode_handle(raw));
    }
}
