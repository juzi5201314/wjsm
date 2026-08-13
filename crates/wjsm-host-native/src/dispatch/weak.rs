use std::collections::{HashMap, HashSet, VecDeque};

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::modules;
use super::promise::{self, NativeMicrotask};
use super::runtime::fail_dispatch;
use crate::{NativeAgentState, NativeCallableKind};

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

fn construct_weak_map(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(object) = weak_object(state) else {
        return fail_dispatch(ctx);
    };
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

pub(crate) fn collect(
    ctx: &NativeVmContext,
    state: &mut NativeAgentState,
    frame_roots: impl IntoIterator<Item = i64>,
) -> HashSet<u32> {
    let mut reachable = strong_reachable(ctx, state, frame_roots);
    close_ephemerons(state, &mut reachable);
    for target in state.weak.weak_refs.values_mut() {
        if target.is_some_and(|target| !target_is_reachable(&reachable, target)) {
            *target = None;
        }
    }
    for entries in state.weak.weak_maps.values_mut() {
        entries.retain(|(target, _)| target_is_reachable(&reachable, *target));
    }
    for values in state.weak.weak_sets.values_mut() {
        values.retain(|target| target_is_reachable(&reachable, *target));
    }

    let mut cleanup_jobs = Vec::new();
    for registry in state.weak.finalization_registries.values_mut() {
        let callback = registry.callback;
        registry.cells.retain(|cell| {
            if target_is_reachable(&reachable, cell.target) {
                true
            } else {
                cleanup_jobs.push((callback, cell.held_value));
                false
            }
        });
    }
    for (callback, held_value) in cleanup_jobs {
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
    reachable
}

fn strong_reachable(
    ctx: &NativeVmContext,
    state: &NativeAgentState,
    frame_roots: impl IntoIterator<Item = i64>,
) -> HashSet<u32> {
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
    queue.extend(state.global_object);
    queue.extend(state.object_prototype);
    queue.extend(state.array_prototype);
    queue.extend(state.regexp_prototype);
    queue.extend(state.console_object);
    queue.extend(state.process_object);
    queue.extend(state.process_env_object);
    queue.extend(state.agent_bridge);
    queue.extend(state.async_generator_prototype);
    queue.extend(state.async_iterator_prototype);
    queue.extend(state.error_prototypes.values().copied());
    queue.extend(state.callable_properties.values().copied());
    queue.extend(
        state
            .callable_accessors
            .values()
            .flat_map(|(getter, setter)| [*getter, *setter]),
    );
    for activation in &state.activations {
        queue.push_back(activation.environment);
        queue.push_back(activation.new_target);
        queue.extend(activation.saved_variables.iter().map(|(_, stored)| *stored));
    }

    let mut reachable = HashSet::new();
    trace_queue(state, &mut queue, &mut reachable);
    reachable
}

fn trace_queue(state: &NativeAgentState, queue: &mut VecDeque<i64>, reachable: &mut HashSet<u32>) {
    while let Some(encoded) = queue.pop_front() {
        if value::is_object(encoded) || value::is_array(encoded) {
            let handle = value::decode_handle(encoded);
            if !reachable.insert(handle) {
                continue;
            }
            if let Ok(references) = state.heap.object_references(handle) {
                queue.extend(references);
            }
            queue.extend(
                state
                    .array_properties
                    .iter()
                    .filter(|((owner, _), _)| *owner == handle)
                    .map(|(_, stored)| *stored),
            );
            if let Some(entries) = state.maps.get(&handle) {
                queue.extend(entries.iter().flat_map(|(key, stored)| [*key, *stored]));
            }
            if let Some(values) = state.sets.get(&handle) {
                queue.extend(values.iter().copied());
            }
            if let Some(array) = state.typed_arrays.get(&handle)
                && let Some(buffer) = array.buffer_object
            {
                queue.push_back(buffer);
            }
            if let Some(view) = state.data_views.get(&handle) {
                queue.push_back(value::encode_object_handle(view.buffer));
            }
            if let Some(buffer) = state.buffers.get(&handle) {
                queue.push_back(buffer.array_buffer);
            }
            if let Some(registry) = state.weak.finalization_registries.get(&handle) {
                queue.push_back(registry.callback);
                queue.extend(registry.cells.iter().map(|cell| cell.held_value));
                queue.extend(
                    registry
                        .cells
                        .iter()
                        .filter_map(|cell| cell.unregister_token),
                );
            }
            if let Some(primitive) = state.boxed_primitives.get(&handle) {
                queue.push_back(*primitive);
            }
        } else if value::is_closure(encoded) {
            if let Some(closure) = state.closures.get(value::decode_handle(encoded) as usize) {
                queue.push_back(closure.environment);
            }
        } else if value::is_bound(encoded) {
            if let Some(bound) = state
                .bound_functions
                .get(value::decode_handle(encoded) as usize)
            {
                queue.extend([bound.target, bound.this_value]);
                queue.extend(bound.arguments.iter().copied());
            }
        } else if value::is_proxy(encoded) {
            if let Some(proxy) = state.proxies.get(value::decode_handle(encoded) as usize) {
                queue.extend([proxy.target, proxy.handler]);
            }
        } else if value::is_native_callable(encoded) {
            queue.extend(
                state
                    .callable_properties
                    .iter()
                    .filter(|((owner, _), _)| *owner == encoded)
                    .map(|(_, stored)| *stored),
            );
            queue.extend(
                state
                    .callable_accessors
                    .iter()
                    .filter(|((owner, _), _)| *owner == encoded)
                    .flat_map(|(_, (getter, setter))| [*getter, *setter]),
            );
            if let Some(prototype) = state.callable_prototypes.get(&encoded) {
                queue.push_back(*prototype);
            }
            if let Some(kind) = state.native_callable_kind(encoded) {
                trace_native_callable(state, kind, queue);
            }
        } else if value::is_exception(encoded)
            && let Some(exception) = state
                .exceptions
                .get(value::decode_handle(encoded) as usize)
                .copied()
        {
            queue.push_back(exception);
        }
    }
}

fn trace_native_callable(
    state: &NativeAgentState,
    kind: NativeCallableKind,
    queue: &mut VecDeque<i64>,
) {
    match kind {
        NativeCallableKind::Bound(index) => {
            if let Some(bound) = state.bound_functions.get(index as usize) {
                queue.extend([bound.target, bound.this_value]);
                queue.extend(bound.arguments.iter().copied());
            }
        }
        NativeCallableKind::PromiseResolve(handle) | NativeCallableKind::PromiseReject(handle) => {
            queue.push_back(value::encode_object_handle(handle));
        }
        NativeCallableKind::ProxyCall(handle) | NativeCallableKind::ProxyConstruct(handle) => {
            queue.push_back(value::encode_proxy_handle(handle));
        }
        _ => {}
    }
}

fn close_ephemerons(state: &NativeAgentState, reachable: &mut HashSet<u32>) {
    loop {
        let mut queue = VecDeque::new();
        for (owner, entries) in &state.weak.weak_maps {
            if !reachable.contains(owner) {
                continue;
            }
            for (key, stored) in entries {
                if target_is_reachable(reachable, *key) {
                    queue.push_back(*stored);
                }
            }
        }
        let before = reachable.len();
        trace_queue(state, &mut queue, reachable);
        if reachable.len() == before {
            break;
        }
    }
}

fn target_is_reachable(reachable: &HashSet<u32>, target: i64) -> bool {
    reachable.contains(&value::decode_handle(target))
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
    let length = state.heap.array_length(handle).ok()?;
    (0..length)
        .map(|index| {
            state
                .heap
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
