use std::collections::{HashMap, VecDeque};

use wjsm_gc::{CycleKind, GcEdge, GcEphemeron, RootSnapshot, RuntimeGcReport};
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::modules;
use super::promise::{self, NativeMicrotask, NativePromiseReaction, PromiseState};
use super::runtime::fail_dispatch;
use crate::side_tables::HostLiveSet;
use crate::{BUILTIN_PROTOTYPE_PROPERTY_FLAGS, NativeAgentState, NativeCallableKind};

#[derive(Clone, Copy)]
struct FinalizationCell {
    target: i64,
    held_value: i64,
    unregister_token: Option<i64>,
}

pub(crate) struct NativeFinalizationRegistry {
    callback: i64,
    cells: Vec<FinalizationCell>,
}

#[derive(Default)]
pub(crate) struct NativeWeakState {
    weak_maps: HashMap<u32, Vec<(i64, i64)>>,
    weak_sets: HashMap<u32, Vec<i64>>,
    weak_refs: HashMap<u32, Option<i64>>,
    finalization_registries: HashMap<u32, NativeFinalizationRegistry>,
}

impl NativeWeakState {
    pub(crate) fn clear(&mut self) {
        self.weak_maps.clear();
        self.weak_sets.clear();
        self.weak_refs.clear();
        self.finalization_registries.clear();
    }

    pub(crate) fn retain_live_owners(&mut self, mut is_live: impl FnMut(u32) -> bool) {
        self.weak_maps.retain(|handle, _| is_live(*handle));
        self.weak_sets.retain(|handle, _| is_live(*handle));
        self.weak_refs.retain(|handle, _| is_live(*handle));
        self.finalization_registries
            .retain(|handle, _| is_live(*handle));
    }
}

pub(super) fn dispatch_weak(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::WeakMapConstructor => construct_weak_map(ctx, state, args),
        Builtin::WeakMapProtoSet => weak_map_set(ctx, state, args),
        Builtin::WeakMapProtoGet => weak_map_get(ctx, state, args),
        Builtin::WeakMapProtoHas => weak_map_has(ctx, state, args),
        Builtin::WeakMapProtoDelete => weak_map_delete(ctx, state, args),
        Builtin::WeakSetConstructor => construct_weak_set(ctx, state, args),
        Builtin::WeakSetProtoAdd => weak_set_add(ctx, state, args),
        Builtin::WeakSetProtoHas => weak_set_has(ctx, state, args),
        Builtin::WeakSetProtoDelete => weak_set_delete(ctx, state, args),
        Builtin::WeakRefConstructor => construct_weak_ref(ctx, state, args),
        Builtin::WeakRefProtoDeref => weak_ref_deref(ctx, state, args),
        Builtin::FinalizationRegistryConstructor => {
            construct_finalization_registry(ctx, state, args)
        }
        Builtin::FinalizationRegistryProtoRegister => {
            finalization_registry_register(ctx, state, args)
        }
        Builtin::FinalizationRegistryProtoUnregister => {
            finalization_registry_unregister(ctx, state, args)
        }
        _ => return None,
    })
}

pub(crate) fn property(state: &NativeAgentState, receiver: i64, key: &str) -> Option<Builtin> {
    let handle = value::decode_handle(receiver);
    if state.weak.weak_maps.contains_key(&handle) {
        return Some(match key {
            "delete" => Builtin::WeakMapProtoDelete,
            "get" => Builtin::WeakMapProtoGet,
            "has" => Builtin::WeakMapProtoHas,
            "set" => Builtin::WeakMapProtoSet,
            _ => return None,
        });
    }
    if state.weak.weak_sets.contains_key(&handle) {
        return Some(match key {
            "add" => Builtin::WeakSetProtoAdd,
            "delete" => Builtin::WeakSetProtoDelete,
            "has" => Builtin::WeakSetProtoHas,
            _ => return None,
        });
    }
    if state.weak.weak_refs.contains_key(&handle) {
        return (key == "deref").then_some(Builtin::WeakRefProtoDeref);
    }
    if state.weak.finalization_registries.contains_key(&handle) {
        return Some(match key {
            "register" => Builtin::FinalizationRegistryProtoRegister,
            "unregister" => Builtin::FinalizationRegistryProtoUnregister,
            _ => return None,
        });
    }
    None
}

pub(crate) fn install_prototype_methods(
    state: &mut NativeAgentState,
    prototype: i64,
    is_set: bool,
) -> Result<(), ()> {
    let methods: &[(&str, Builtin)] = if is_set {
        &[
            ("add", Builtin::WeakSetProtoAdd),
            ("delete", Builtin::WeakSetProtoDelete),
            ("has", Builtin::WeakSetProtoHas),
        ]
    } else {
        &[
            ("delete", Builtin::WeakMapProtoDelete),
            ("get", Builtin::WeakMapProtoGet),
            ("has", Builtin::WeakMapProtoHas),
            ("set", Builtin::WeakMapProtoSet),
        ]
    };
    let prototype = value::decode_handle(prototype);
    for &(name, builtin) in methods {
        let key = state.intern_property_string(name.into()).ok_or(())?;
        let callable = state
            .native_callable(NativeCallableKind::Builtin(builtin, true))
            .ok_or(())?;
        state
            .gc
            .heap()
            .set_property(prototype, key, callable as u64)
            .map_err(|_| ())?;
        state
            .gc
            .heap()
            .update_property_flags(prototype, key, BUILTIN_PROTOTYPE_PROPERTY_FLAGS)
            .map_err(|_| ())?;
    }
    Ok(())
}

fn construct_weak_map(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(object) = weak_object(state) else {
        return fail_dispatch(ctx);
    };
    if state
        .set_collection_prototype(object, Builtin::WeakMapConstructor)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    state
        .weak
        .weak_maps
        .insert(value::decode_handle(object), Vec::new());
    if let Some(iterable) = args
        .first()
        .copied()
        .filter(|input| value::is_array(*input))
    {
        let Some(entries) = array_values(state, iterable) else {
            return type_error(ctx, state, "WeakMap iterable is invalid");
        };
        for entry in entries {
            let Some(pair) = array_values(state, entry).filter(|pair| pair.len() >= 2) else {
                return type_error(ctx, state, "WeakMap iterable entry is invalid");
            };
            if !is_weak_target(pair[0]) {
                return type_error(ctx, state, "Invalid value used as weak map key");
            }
            weak_map_insert(state, object, pair[0], pair[1]);
        }
    }
    object
}

fn construct_weak_set(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(object) = weak_object(state) else {
        return fail_dispatch(ctx);
    };
    if state
        .set_collection_prototype(object, Builtin::WeakSetConstructor)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    state
        .weak
        .weak_sets
        .insert(value::decode_handle(object), Vec::new());
    if let Some(iterable) = args
        .first()
        .copied()
        .filter(|input| value::is_array(*input))
    {
        let Some(values) = array_values(state, iterable) else {
            return type_error(ctx, state, "WeakSet iterable is invalid");
        };
        for target in values {
            if !is_weak_target(target) {
                return type_error(ctx, state, "Invalid value used in weak set");
            }
            weak_set_insert(state, object, target);
        }
    }
    object
}

fn construct_weak_ref(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(target) = args
        .first()
        .copied()
        .filter(|target| is_weak_target(*target))
    else {
        return type_error(ctx, state, "WeakRef target must be an object");
    };
    let Some(object) = weak_object(state) else {
        return fail_dispatch(ctx);
    };
    state
        .weak
        .weak_refs
        .insert(value::decode_handle(object), Some(target));
    object
}

fn construct_finalization_registry(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(callback) = args
        .first()
        .copied()
        .filter(|callback| value::is_callable(*callback))
    else {
        return type_error(ctx, state, "cleanup callback must be callable");
    };
    let Some(object) = weak_object(state) else {
        return fail_dispatch(ctx);
    };
    state.weak.finalization_registries.insert(
        value::decode_handle(object),
        NativeFinalizationRegistry {
            callback,
            cells: Vec::new(),
        },
    );
    object
}

fn weak_map_set(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, key, stored] = args else {
        return fail_dispatch(ctx);
    };
    if !is_weak_target(*key) {
        return type_error(ctx, state, "Invalid value used as weak map key");
    }
    if !state
        .weak
        .weak_maps
        .contains_key(&value::decode_handle(*receiver))
    {
        return type_error(ctx, state, "WeakMap method called on incompatible receiver");
    }
    weak_map_insert(state, *receiver, *key, *stored);
    *receiver
}

fn weak_map_get(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, key] = args else {
        return fail_dispatch(ctx);
    };
    state
        .weak
        .weak_maps
        .get(&value::decode_handle(*receiver))
        .and_then(|entries| entries.iter().find(|(candidate, _)| *candidate == *key))
        .map_or_else(value::encode_undefined, |(_, stored)| *stored)
}

fn weak_map_has(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, key] = args else {
        return fail_dispatch(ctx);
    };
    value::encode_bool(
        state
            .weak
            .weak_maps
            .get(&value::decode_handle(*receiver))
            .is_some_and(|entries| entries.iter().any(|(candidate, _)| *candidate == *key)),
    )
}

fn weak_map_delete(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, key] = args else {
        return fail_dispatch(ctx);
    };
    let Some(entries) = state
        .weak
        .weak_maps
        .get_mut(&value::decode_handle(*receiver))
    else {
        return type_error(ctx, state, "WeakMap method called on incompatible receiver");
    };
    let before = entries.len();
    entries.retain(|(candidate, _)| *candidate != *key);
    value::encode_bool(entries.len() != before)
}

fn weak_set_add(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, target] = args else {
        return fail_dispatch(ctx);
    };
    if !is_weak_target(*target) {
        return type_error(ctx, state, "Invalid value used in weak set");
    }
    if !state
        .weak
        .weak_sets
        .contains_key(&value::decode_handle(*receiver))
    {
        return type_error(ctx, state, "WeakSet method called on incompatible receiver");
    }
    weak_set_insert(state, *receiver, *target);
    *receiver
}

fn weak_set_has(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, target] = args else {
        return fail_dispatch(ctx);
    };
    value::encode_bool(
        state
            .weak
            .weak_sets
            .get(&value::decode_handle(*receiver))
            .is_some_and(|values| values.contains(target)),
    )
}

fn weak_set_delete(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver, target] = args else {
        return fail_dispatch(ctx);
    };
    let Some(values) = state
        .weak
        .weak_sets
        .get_mut(&value::decode_handle(*receiver))
    else {
        return type_error(ctx, state, "WeakSet method called on incompatible receiver");
    };
    let before = values.len();
    values.retain(|candidate| *candidate != *target);
    value::encode_bool(values.len() != before)
}

fn weak_ref_deref(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [receiver] = args else {
        return fail_dispatch(ctx);
    };
    state
        .weak
        .weak_refs
        .get(&value::decode_handle(*receiver))
        .copied()
        .flatten()
        .unwrap_or_else(value::encode_undefined)
}

fn finalization_registry_register(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [receiver, target, held_value, rest @ ..] = args else {
        return fail_dispatch(ctx);
    };
    if !is_weak_target(*target) {
        return type_error(ctx, state, "FinalizationRegistry target must be an object");
    }
    if *target == *held_value {
        return type_error(ctx, state, "target and holdings must not be the same");
    }
    let unregister_token = rest
        .first()
        .copied()
        .filter(|token| !value::is_undefined(*token));
    if unregister_token.is_some_and(|token| !is_weak_target(token)) {
        return type_error(ctx, state, "unregister token must be an object");
    }
    let Some(registry) = state
        .weak
        .finalization_registries
        .get_mut(&value::decode_handle(*receiver))
    else {
        return type_error(
            ctx,
            state,
            "FinalizationRegistry method called on incompatible receiver",
        );
    };
    registry.cells.push(FinalizationCell {
        target: *target,
        held_value: *held_value,
        unregister_token,
    });
    value::encode_undefined()
}

fn finalization_registry_unregister(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [receiver, token] = args else {
        return fail_dispatch(ctx);
    };
    if !is_weak_target(*token) {
        return type_error(ctx, state, "unregister token must be an object");
    }
    let Some(registry) = state
        .weak
        .finalization_registries
        .get_mut(&value::decode_handle(*receiver))
    else {
        return type_error(
            ctx,
            state,
            "FinalizationRegistry method called on incompatible receiver",
        );
    };
    let before = registry.cells.len();
    registry
        .cells
        .retain(|cell| cell.unregister_token != Some(*token));
    value::encode_bool(registry.cells.len() != before)
}

pub(crate) fn snapshot_gc_graph(
    ctx: &NativeVmContext,
    state: &NativeAgentState,
    frame_roots: impl IntoIterator<Item = i64>,
    epoch: u64,
) -> RootSnapshot {
    let roots = root_values(ctx, state, frame_roots)
        .into_iter()
        .chain(state.property_name_handles())
        .collect();
    let (strong_edges, ephemerons) = host_edges(state);
    RootSnapshot::new(epoch, roots, strong_edges, ephemerons)
}

pub(crate) fn finish_gc_cycle(state: &mut NativeAgentState, report: &RuntimeGcReport) {
    let mut live_host_values = report.live_host_values.clone();
    let retired = &report.retired_handles;
    for target in state.weak.weak_refs.values_mut() {
        if target.is_some_and(|target| retired.binary_search(&value::decode_handle(target)).is_ok())
        {
            *target = None;
        }
    }
    for entries in state.weak.weak_maps.values_mut() {
        entries.retain(|(target, _)| {
            retired
                .binary_search(&value::decode_handle(*target))
                .is_err()
        });
    }
    for values in state.weak.weak_sets.values_mut() {
        values.retain(|target| {
            retired
                .binary_search(&value::decode_handle(*target))
                .is_err()
        });
    }

    let mut cleanup_jobs = Vec::new();
    for registry in state.weak.finalization_registries.values_mut() {
        let callback = registry.callback;
        registry.cells.retain(|cell| {
            if retired
                .binary_search(&value::decode_handle(cell.target))
                .is_err()
            {
                true
            } else {
                cleanup_jobs.push((callback, cell.held_value));
                false
            }
        });
    }
    for (callback, held_value) in cleanup_jobs {
        live_host_values.extend([callback, held_value]);
        promise::enqueue_microtask(
            state,
            NativeMicrotask::Callback {
                callback,
                arguments: vec![held_value],
                resource: None,
                repeat: false,
            },
        );
    }
    let live = host_live_set(state, &live_host_values);
    state.cleanup_retired_handles(retired);
    state.prune_string_ids(retired);
    if report.stats.cycle_kind == CycleKind::Full
        && state.runtime_config.gc_algorithm == wjsm_gc::GcAlgorithmKind::Zgc
    {
        state.prune_unmarked_string_ids();
    }
    if report.cleans_host_tables {
        state.sweep_host_index_tables(retired, &live);
    }
}

fn root_values(
    ctx: &NativeVmContext,
    state: &NativeAgentState,
    frame_roots: impl IntoIterator<Item = i64>,
) -> Vec<i64> {
    let mut queue = VecDeque::new();
    queue.extend(frame_roots);
    queue.extend(state.variables.iter().copied());
    queue.extend(state.isolated_variable_tables.values().flatten().copied());
    if let Some(shared) = &state.shared_variables_backup {
        queue.extend(shared.iter().copied());
    }
    queue.extend(
        state.call_arena[..usize::try_from(ctx.call_arena_active_len).unwrap_or(0)]
            .iter()
            .copied(),
    );
    queue.extend(state.latin1_char_strings.iter().copied());
    queue.extend(state.global_object);
    queue.extend(state.object_prototype);
    queue.extend(state.array_prototype);
    queue.extend(state.regexp_prototype);
    queue.extend(state.map_prototype);
    queue.extend(state.set_prototype);
    queue.extend(state.weak_map_prototype);
    queue.extend(state.weak_set_prototype);
    queue.extend(state.console_object);
    queue.extend(state.intl.object);
    queue.extend(state.intl.locale_prototype);
    queue.extend(state.intl.collator_prototype);
    queue.extend(state.intl.number_format_prototype);
    queue.extend(state.intl.datetime_format_prototype);
    queue.extend(state.intl.plural_rules_prototype);
    queue.extend(state.intl.list_format_prototype);
    queue.extend(state.intl.relative_time_prototype);
    queue.extend(state.intl.display_names_prototype);
    queue.extend(state.intl.segmenter_prototype);
    queue.extend(state.intl.segments_prototype);
    queue.extend(state.intl.segment_iterator_prototype);
    queue.extend(state.intl.duration_format_prototype);
    queue.extend(state.intl.string_prototype);
    queue.extend(state.intl.number_prototype);
    queue.extend(state.intl.bigint_prototype);
    queue.extend(state.process_object);
    queue.extend(state.process_env_object);
    queue.extend(state.agent_bridge);
    queue.extend(state.async_generator_prototype);
    queue.extend(state.async_iterator_prototype);
    queue.extend(state.error_prototypes.values().copied());
    queue.extend(state.out_of_memory_error);
    queue.extend(state.out_of_memory_exception);
    for activation in &state.activations {
        queue.push_back(activation.environment);
        queue.push_back(activation.new_target);
        queue.extend(activation.saved_variables.iter().map(|(_, stored)| *stored));
    }
    // materialized_constants 持有的字符串/闭包下标必须在回收后保持存活，
    // 否则槽位复用后常量别名到错误值。当前 image 的常量在 state 上，其余已加载
    // image 的常量在 programs 里，都要钉扎。
    queue.extend(state.materialized_constants.iter().copied().flatten());
    for program in state.programs.values() {
        queue.extend(program.materialized_constants.iter().copied().flatten());
    }
    // 宿主侧持久的 JS 值（微任务/计时器/挂起 promise/continuation/回调等）同样
    // 是根：下标表回收后，仍被这些结构引用的闭包/字符串不得被 tombstone。
    extend_host_roots(state, &mut queue);

    queue.into_iter().collect()
}
fn host_edges(state: &NativeAgentState) -> (Vec<GcEdge>, Vec<GcEphemeron>) {
    let mut edges = Vec::new();
    let mut ephemerons = Vec::new();
    let mut add = |owner: i64, target: i64| edges.push(GcEdge { owner, target });
    let owner = |handle| object_owner(state, handle);
    for ((handle, _), stored) in &state.array_properties {
        add(owner(*handle), *stored);
    }
    for ((handle, _), (getter, setter, _)) in &state.array_accessors {
        add(owner(*handle), *getter);
        add(owner(*handle), *setter);
    }
    for (handle, entries) in &state.maps {
        for (key, value) in entries {
            add(owner(*handle), *key);
            add(owner(*handle), *value);
        }
    }
    for (handle, values) in &state.sets {
        for value in values {
            add(owner(*handle), *value);
        }
    }
    for (handle, slot) in &state.intl.slots {
        for bound in slot.bound_roots() {
            add(owner(*handle), bound);
        }
    }
    for (handle, array) in &state.typed_arrays {
        let array_owner = owner(*handle);
        if let Some(buffer) = array.buffer_object {
            add(array_owner, buffer);
        }
        if let Some(storage) = &array.storage {
            for stored in storage.borrow().iter().copied() {
                add(array_owner, stored);
            }
        }
    }
    for (handle, view) in &state.data_views {
        add(owner(*handle), value::encode_object_handle(view.buffer));
    }
    for (handle, buffer) in &state.buffers {
        add(owner(*handle), buffer.array_buffer);
    }
    for (handle, registry) in &state.weak.finalization_registries {
        let registry_owner = owner(*handle);
        add(registry_owner, registry.callback);
        for cell in &registry.cells {
            add(registry_owner, cell.held_value);
            if let Some(token) = cell.unregister_token {
                add(registry_owner, token);
            }
        }
    }
    for (handle, primitive) in &state.boxed_primitives {
        add(owner(*handle), *primitive);
    }
    for ((owner, _), value) in &state.callable_properties {
        add(*owner, *value);
    }
    for ((owner, _), (getter, setter)) in &state.callable_accessors {
        add(*owner, *getter);
        add(*owner, *setter);
    }
    for (owner, prototype) in &state.callable_prototypes {
        add(*owner, *prototype);
    }
    for (index, callable) in state.native_callables.iter().enumerate() {
        let owner = value::encode_native_callable_idx(index as u32);
        match callable {
            NativeCallableKind::Bound(index) => {
                if let Some(bound) = state
                    .bound_functions
                    .get(*index as usize)
                    .and_then(Option::as_ref)
                {
                    add(owner, bound.target);
                    add(owner, bound.this_value);
                    for argument in &bound.arguments {
                        add(owner, *argument);
                    }
                }
            }
            NativeCallableKind::PromiseResolve(handle)
            | NativeCallableKind::PromiseReject(handle) => {
                add(owner, value::encode_object_handle(*handle));
            }
            NativeCallableKind::ProxyCall(handle) | NativeCallableKind::ProxyConstruct(handle) => {
                add(owner, value::encode_proxy_handle(*handle));
            }
            _ => {}
        }
    }
    for (owner, exception) in state.exceptions.iter().enumerate() {
        if let Some(exception) = exception {
            add(value::encode_exception(owner as u32), *exception);
        }
    }
    for ((owner, _), slot) in &state.private_slots {
        match slot {
            crate::NativePrivateSlot::Data(stored) => add(*owner, *stored),
            crate::NativePrivateSlot::Accessor { getter, setter } => {
                add(*owner, *getter);
                add(*owner, *setter);
            }
        }
    }
    for (handle, entries) in &state.weak.weak_maps {
        let owner = object_owner(state, *handle);
        for (key, value) in entries {
            ephemerons.push(GcEphemeron {
                owner,
                key: *key,
                value: *value,
            });
        }
    }
    for (owner, closure) in state.closures.iter().enumerate() {
        if let Some(closure) = closure {
            add(value::encode_closure_idx(owner as u32), closure.environment);
        }
    }
    for (owner, bound) in state.bound_functions.iter().enumerate() {
        if let Some(bound) = bound {
            let encoded = value::encode_bound_idx(owner as u32);
            add(encoded, bound.target);
            add(encoded, bound.this_value);
            for argument in &bound.arguments {
                add(encoded, *argument);
            }
        }
    }
    for (owner, proxy) in state.proxies.iter().enumerate() {
        if let Some(proxy) = proxy {
            add(value::encode_proxy_handle(owner as u32), proxy.target);
            add(value::encode_proxy_handle(owner as u32), proxy.handler);
        }
    }
    (edges, ephemerons)
}

fn object_owner(state: &NativeAgentState, handle: u32) -> i64 {
    if state.gc.heap().object_type(handle).ok() == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY)) {
        value::encode_handle(value::TAG_ARRAY, handle)
    } else {
        value::encode_object_handle(handle)
    }
}

fn host_live_set(state: &NativeAgentState, values: &[i64]) -> HostLiveSet {
    let mut live = HostLiveSet::default();
    for encoded in values {
        if value::is_closure(*encoded) {
            live.closures.insert(value::decode_closure_idx(*encoded));
        } else if value::is_bound(*encoded) {
            live.bound.insert(value::decode_bound_idx(*encoded));
        } else if value::is_proxy(*encoded) {
            live.proxies.insert(value::decode_proxy_handle(*encoded));
        } else if value::is_native_callable(*encoded) {
            match state.native_callable_kind(*encoded) {
                Some(NativeCallableKind::Bound(index)) => {
                    live.bound.insert(index);
                }
                Some(
                    NativeCallableKind::ProxyRevoke(index)
                    | NativeCallableKind::ProxyCall(index)
                    | NativeCallableKind::ProxyConstruct(index),
                ) => {
                    live.proxies.insert(index);
                }
                _ => {}
            }
        } else if value::is_regexp(*encoded) {
            live.regexps.insert(value::decode_regexp_handle(*encoded));
        } else if value::is_exception(*encoded) {
            live.exceptions.insert(value::decode_handle(*encoded));
        }
    }
    live
}

/// 把宿主侧持久 JS 值（微任务、计时器、挂起 promise、continuation、generator、
/// async hooks / perf hooks 回调等）并入根队列。它们经 i64 下标/值引用闭包与
/// 字符串，回收前必须钉扎。
fn extend_host_roots(state: &NativeAgentState, queue: &mut VecDeque<i64>) {
    for scheduled in state
        .microtasks
        .iter()
        .chain(state.next_ticks.iter())
        .chain(state.immediates.iter())
    {
        extend_microtask_roots(&scheduled.task, queue);
    }
    for timer in &state.timers {
        extend_microtask_roots(&timer.scheduled.task, queue);
    }
    for promise in state.promises.values() {
        match promise.state {
            PromiseState::Fulfilled(value) | PromiseState::Rejected(value) => {
                queue.push_back(value);
            }
            PromiseState::Pending => {}
        }
    }
    for reactions in state.promise_reactions.values() {
        for scheduled in reactions {
            extend_reaction_roots(&scheduled.reaction, queue);
        }
    }
    for combinator in &state.promise_combinators {
        queue.extend(combinator.values.iter().copied());
    }
    for continuation in state.continuations.values() {
        queue.push_back(continuation.function);
        queue.push_back(continuation.outer_promise);
        queue.extend(continuation.vars.iter().copied());
    }
    for generator in state.generators.values() {
        queue.push_back(generator.continuation);
    }
    for generator in state.async_generators.values() {
        queue.push_back(generator.continuation);
        if let Some(request) = &generator.active {
            queue.push_back(request.value);
            queue.push_back(request.promise);
        }
        for request in &generator.queue {
            queue.push_back(request.value);
            queue.push_back(request.promise);
        }
        queue.extend(generator.resume_promise);
    }
    for iterator in state.array_iterators.values() {
        match iterator.source {
            crate::NativeIteratorSource::String(encoded)
            | crate::NativeIteratorSource::Custom(encoded) => queue.push_back(encoded),
            crate::NativeIteratorSource::Array(handle) => {
                queue.push_back(value::encode_handle(value::TAG_ARRAY, handle));
            }
            crate::NativeIteratorSource::ArrayLike(handle)
            | crate::NativeIteratorSource::TypedArray(handle)
            | crate::NativeIteratorSource::Map(handle)
            | crate::NativeIteratorSource::Set(handle) => {
                queue.push_back(value::encode_object_handle(handle));
            }
        }
        if let Some(current) = iterator.current {
            queue.push_back(current);
        }
    }
    queue.extend(state.async_from_sync_iterators.values().copied());
    state
        .runtime_modules
        .visit_gc_roots(|root| queue.push_back(root));
    queue.extend(state.fatal_exception);
    state.node_perf_hooks.extend_gc_roots(queue);
    queue.extend(state.node_async_hooks.defaults.values().copied());
    for stores in state.node_async_hooks.captured_frames.iter().flatten() {
        queue.extend(stores.values().copied());
    }
    queue.push_back(state.node_async_hooks.top_resource);
    if let Some(stores) = &state.node_async_hooks.current.stores {
        queue.extend(stores.values().copied());
    }
    queue.push_back(state.node_async_hooks.current.resource);
    for snapshot in &state.node_async_hooks.execution_stack {
        if let Some(stores) = &snapshot.stores {
            queue.extend(stores.values().copied());
        }
        queue.push_back(snapshot.resource);
    }
    for resource in state.node_async_hooks.resources.values() {
        queue.push_back(resource.resource);
        if let Some(stores) = &resource.stores {
            queue.extend(stores.values().copied());
        }
    }
    for hook in &state.node_async_hooks.hooks {
        queue.push_back(hook.init);
        queue.push_back(hook.before);
        queue.push_back(hook.after);
        queue.push_back(hook.destroy);
        queue.push_back(hook.promise_resolve);
    }
    for event in &state.node_async_hooks.pending_events {
        queue.extend(event.args.iter().copied());
    }
}

fn extend_microtask_roots(task: &NativeMicrotask, queue: &mut VecDeque<i64>) {
    match task {
        NativeMicrotask::Callback {
            callback,
            arguments,
            resource,
            ..
        } => {
            queue.push_back(*callback);
            queue.extend(arguments.iter().copied());
            queue.extend(*resource);
        }
        NativeMicrotask::PromiseReaction {
            reaction, value, ..
        } => {
            extend_reaction_roots(reaction, queue);
            queue.push_back(*value);
        }
        NativeMicrotask::DynamicImport { .. } => {}
        NativeMicrotask::AsyncResume {
            continuation,
            state,
            value,
            ..
        } => {
            queue.push_back(*continuation);
            queue.push_back(*state);
            queue.push_back(*value);
        }
        NativeMicrotask::ResolveThenable { thenable, then, .. } => {
            queue.push_back(*thenable);
            queue.push_back(*then);
        }
        NativeMicrotask::Stream(stream) => {
            if let super::streams::StreamTask::Write { chunk, .. } = stream {
                queue.push_back(*chunk);
            }
        }
    }
}

fn extend_reaction_roots(reaction: &NativePromiseReaction, queue: &mut VecDeque<i64>) {
    match reaction {
        NativePromiseReaction::Handler {
            on_fulfilled,
            on_rejected,
            ..
        } => {
            queue.push_back(*on_fulfilled);
            queue.push_back(*on_rejected);
        }
        NativePromiseReaction::AsyncResume {
            continuation,
            state,
        } => {
            queue.push_back(*continuation);
            queue.push_back(*state);
        }
        NativePromiseReaction::CombinatorElement { .. } => {}
        NativePromiseReaction::Finally { callback, .. } => {
            queue.push_back(*callback);
        }
        NativePromiseReaction::FinallyResult { original, .. } => {
            queue.push_back(*original);
        }
        NativePromiseReaction::Stream(_) => {}
    }
}

fn weak_map_insert(state: &mut NativeAgentState, receiver: i64, key: i64, stored: i64) {
    let entries = state
        .weak
        .weak_maps
        .get_mut(&value::decode_handle(receiver))
        .expect("WeakMap receiver was validated");
    if let Some((_, current)) = entries.iter_mut().find(|(candidate, _)| *candidate == key) {
        *current = stored;
    } else {
        entries.push((key, stored));
    }
}

fn weak_set_insert(state: &mut NativeAgentState, receiver: i64, target: i64) {
    let values = state
        .weak
        .weak_sets
        .get_mut(&value::decode_handle(receiver))
        .expect("WeakSet receiver was validated");
    if !values.contains(&target) {
        values.push(target);
    }
}

fn weak_object(state: &NativeAgentState) -> Option<i64> {
    state.allocate_object(2, false).ok()
}

fn array_values(state: &NativeAgentState, encoded: i64) -> Option<Vec<i64>> {
    let handle = value::decode_handle(encoded);
    let length = state.gc.heap().array_length(handle).ok()?;
    (0..length)
        .map(|index| {
            state
                .gc
                .heap()
                .get_element(handle, index)
                .ok()
                .flatten()
                .map(|stored| stored as i64)
        })
        .collect()
}

fn is_weak_target(encoded: i64) -> bool {
    value::is_js_object(encoded)
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    modules::named_error_object(state, "TypeError", message.to_owned())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
