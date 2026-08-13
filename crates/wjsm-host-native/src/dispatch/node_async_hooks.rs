use std::collections::{HashMap, VecDeque};

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{fail_dispatch, modules};
use crate::{NativeAgentState, NativeCallableKind};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodeAsyncHooksCallable {
    AlsCaptureFrame,
    AlsDisable,
    AlsEnterWith,
    AlsGetStore,
    AlsNew,
    AlsPopFrame,
    AlsPushFrame,
    AsyncResourceAsyncId,
    AsyncResourceEmitDestroy,
    AsyncResourceEnter,
    AsyncResourceExit,
    AsyncResourceInit,
    AsyncResourceNew,
    AsyncResourceTriggerAsyncId,
    CreateHook,
    ExecutionAsyncId,
    ExecutionAsyncResource,
    HookDisable(u32),
    HookEnable(u32),
    Providers,
    TriggerAsyncId,
}

#[derive(Clone, Default)]
pub(crate) struct AsyncContextSnapshot {
    pub(crate) stores: Option<HashMap<u32, i64>>,
    execution_async_id: u64,
    trigger_async_id: u64,
    pub(crate) resource: i64,
}

pub(crate) struct NodeAsyncHooksState {
    bridge: Option<i64>,
    next_als_key: u32,
    pub(crate) defaults: HashMap<u32, i64>,
    pub(crate) current: AsyncContextSnapshot,
    pub(crate) captured_frames: Vec<Option<HashMap<u32, i64>>>,
    pub(crate) execution_stack: Vec<AsyncContextSnapshot>,
    next_async_id: u64,
    pub(crate) pending_events: VecDeque<PendingHookEvent>,
    pub(crate) top_resource: i64,
    pub(crate) resources: HashMap<u32, AsyncResourceRecord>,
    pub(crate) hooks: Vec<HookRecord>,
}

impl Default for NodeAsyncHooksState {
    fn default() -> Self {
        Self {
            bridge: None,
            next_als_key: 1,
            defaults: HashMap::new(),
            current: AsyncContextSnapshot {
                stores: None,
                execution_async_id: 1,
                trigger_async_id: 0,
                resource: 0,
            },
            captured_frames: Vec::new(),
            execution_stack: Vec::new(),
            pending_events: VecDeque::new(),
            next_async_id: 2,
            top_resource: 0,
            resources: HashMap::new(),
            hooks: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct AsyncResourceRecord {
    async_id: u64,
    trigger_async_id: u64,
    pub(crate) resource: i64,
    pub(crate) stores: Option<HashMap<u32, i64>>,
    destroyed: bool,
    promise: bool,
    gc_destroy: bool,
}

#[derive(Clone)]
pub(crate) struct HookRecord {
    pub(crate) init: i64,
    pub(crate) before: i64,
    pub(crate) after: i64,
    pub(crate) destroy: i64,
    pub(crate) promise_resolve: i64,
    track_promises: bool,
    enabled: bool,
}

pub(crate) struct PendingHookEvent {
    phase: HookPhase,
    pub(crate) args: Vec<i64>,
    promise: bool,
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_async_hooks.bridge {
        return Some(bridge);
    }
    if state.node_async_hooks.top_resource == 0 {
        let resource = state.allocate_object(0, false).ok()?;
        state.node_async_hooks.top_resource = resource;
        state.node_async_hooks.current.resource = resource;
    }
    let methods = [
        ("executionAsyncId", NodeAsyncHooksCallable::ExecutionAsyncId),
        ("triggerAsyncId", NodeAsyncHooksCallable::TriggerAsyncId),
        (
            "executionAsyncResource",
            NodeAsyncHooksCallable::ExecutionAsyncResource,
        ),
        ("alsNew", NodeAsyncHooksCallable::AlsNew),
        ("alsEnterWith", NodeAsyncHooksCallable::AlsEnterWith),
        ("alsGetStore", NodeAsyncHooksCallable::AlsGetStore),
        ("alsDisable", NodeAsyncHooksCallable::AlsDisable),
        ("asyncResourceNew", NodeAsyncHooksCallable::AsyncResourceNew),
        (
            "asyncResourceEnter",
            NodeAsyncHooksCallable::AsyncResourceEnter,
        ),
        (
            "asyncResourceExit",
            NodeAsyncHooksCallable::AsyncResourceExit,
        ),
        (
            "asyncResourceEmitDestroy",
            NodeAsyncHooksCallable::AsyncResourceEmitDestroy,
        ),
        (
            "asyncResourceInit",
            NodeAsyncHooksCallable::AsyncResourceInit,
        ),
        (
            "asyncResourceAsyncId",
            NodeAsyncHooksCallable::AsyncResourceAsyncId,
        ),
        (
            "asyncResourceTriggerAsyncId",
            NodeAsyncHooksCallable::AsyncResourceTriggerAsyncId,
        ),
        ("providers", NodeAsyncHooksCallable::Providers),
        ("createHook", NodeAsyncHooksCallable::CreateHook),
        ("alsCaptureFrame", NodeAsyncHooksCallable::AlsCaptureFrame),
        ("alsPushFrame", NodeAsyncHooksCallable::AlsPushFrame),
        ("alsPopFrame", NodeAsyncHooksCallable::AlsPopFrame),
    ];
    let bridge = state.allocate_object(methods.len() as u32, false).ok()?;
    for (name, method) in methods {
        let callable = state.native_callable(NativeCallableKind::NodeAsyncHooks(method))?;
        modules::set_named_property(state, bridge, name, callable).ok()?;
    }
    state.node_async_hooks.bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: NodeAsyncHooksCallable,
    this_value: i64,
    args: &[i64],
) -> i64 {
    match callable {
        NodeAsyncHooksCallable::AlsCaptureFrame => capture_frame(state),
        NodeAsyncHooksCallable::AlsDisable => als_disable(state, args),
        NodeAsyncHooksCallable::AlsEnterWith => als_enter_with(state, args),
        NodeAsyncHooksCallable::AlsGetStore => als_get_store(state, args),
        NodeAsyncHooksCallable::AlsNew => als_new(state, args),
        NodeAsyncHooksCallable::AlsPopFrame => als_pop_frame(state, args),
        NodeAsyncHooksCallable::AlsPushFrame => als_push_frame(state, args),
        NodeAsyncHooksCallable::AsyncResourceAsyncId => resource_id(state, args, false),
        NodeAsyncHooksCallable::AsyncResourceEmitDestroy => resource_destroy(ctx, state, args),
        NodeAsyncHooksCallable::AsyncResourceEnter => resource_enter(ctx, state, args),
        NodeAsyncHooksCallable::AsyncResourceExit => resource_exit(ctx, state, args),
        NodeAsyncHooksCallable::AsyncResourceInit => resource_init(ctx, state, args),
        NodeAsyncHooksCallable::AsyncResourceNew => resource_new(ctx, state, args),
        NodeAsyncHooksCallable::AsyncResourceTriggerAsyncId => resource_id(state, args, true),
        NodeAsyncHooksCallable::CreateHook => create_hook(ctx, state, args),
        NodeAsyncHooksCallable::ExecutionAsyncId => {
            value::encode_f64(state.node_async_hooks.current.execution_async_id as f64)
        }
        NodeAsyncHooksCallable::ExecutionAsyncResource => state.node_async_hooks.current.resource,
        NodeAsyncHooksCallable::HookDisable(hook) => {
            set_hook_enabled(state, hook, false, this_value)
        }
        NodeAsyncHooksCallable::HookEnable(hook) => set_hook_enabled(state, hook, true, this_value),
        NodeAsyncHooksCallable::Providers => providers(ctx, state),
        NodeAsyncHooksCallable::TriggerAsyncId => {
            value::encode_f64(state.node_async_hooks.current.trigger_async_id as f64)
        }
    }
}

pub(crate) fn capture_context(state: &NativeAgentState) -> AsyncContextSnapshot {
    state.node_async_hooks.current.clone()
}

pub(crate) fn enter_context(
    state: &mut NativeAgentState,
    snapshot: AsyncContextSnapshot,
) -> AsyncContextSnapshot {
    std::mem::replace(&mut state.node_async_hooks.current, snapshot)
}

pub(crate) fn restore_context(state: &mut NativeAgentState, snapshot: AsyncContextSnapshot) {
    state.node_async_hooks.current = snapshot;
}
pub(crate) fn promise_created(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    promise: i64,
    trigger: Option<u64>,
) -> u64 {
    let async_id = state.node_async_hooks.next_async_id;
    state.node_async_hooks.next_async_id = async_id.saturating_add(1);
    let trigger_async_id = trigger.unwrap_or(state.node_async_hooks.current.execution_async_id);
    state.node_async_hooks.resources.insert(
        value::decode_handle(promise),
        AsyncResourceRecord {
            async_id,
            trigger_async_id,
            resource: promise,
            stores: state.node_async_hooks.current.stores.clone(),
            destroyed: false,
            promise: true,
            gc_destroy: false,
        },
    );
    if let Some(type_value) = state.intern_text("PROMISE".into(), value::TAG_STRING) {
        let _ = emit(
            ctx,
            state,
            HookPhase::Init,
            &[
                value::encode_f64(async_id as f64),
                type_value,
                value::encode_f64(trigger_async_id as f64),
                promise,
            ],
            true,
        );
    }
    async_id
}

pub(crate) fn promise_context(
    state: &NativeAgentState,
    promise: u32,
) -> Option<AsyncContextSnapshot> {
    let resource = state.node_async_hooks.resources.get(&promise)?;
    Some(AsyncContextSnapshot {
        stores: resource.stores.clone(),
        execution_async_id: resource.async_id,
        trigger_async_id: resource.trigger_async_id,
        resource: resource.resource,
    })
}

pub(crate) fn inherit_promise_stores(state: &mut NativeAgentState, target: u32, source: u32) {
    let stores = state
        .node_async_hooks
        .resources
        .get(&source)
        .and_then(|resource| resource.stores.clone());
    if let Some(target) = state.node_async_hooks.resources.get_mut(&target) {
        target.stores = stores;
    }
}

pub(crate) fn promise_settled(state: &mut NativeAgentState, promise: u32) {
    let Some(async_id) = state
        .node_async_hooks
        .resources
        .get(&promise)
        .map(|resource| resource.async_id)
    else {
        return;
    };
    state
        .node_async_hooks
        .pending_events
        .push_back(PendingHookEvent {
            phase: HookPhase::PromiseResolve,
            args: vec![value::encode_f64(async_id as f64)],
            promise: true,
        });
}

pub(crate) fn drain_hook_events(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
) -> Option<i64> {
    while let Some(event) = state.node_async_hooks.pending_events.pop_front() {
        if let Some(exception) = emit(ctx, state, event.phase, &event.args, event.promise) {
            return Some(exception);
        }
    }
    None
}

pub(crate) fn emit_current_phase(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    before: bool,
) -> Option<i64> {
    let current = &state.node_async_hooks.current;
    let record = state
        .node_async_hooks
        .resources
        .get(&value::decode_handle(current.resource))?;
    emit(
        ctx,
        state,
        if before {
            HookPhase::Before
        } else {
            HookPhase::After
        },
        &[value::encode_f64(current.execution_async_id as f64)],
        record.promise,
    )
}

pub(crate) fn create_scheduled_resource(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    type_name: &str,
) -> Result<(i64, AsyncContextSnapshot), i64> {
    let Some(type_value) = state.intern_text(type_name.into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let resource = resource_new(ctx, state, &[type_value]);
    if value::is_exception(resource) {
        return Err(resource);
    }
    if let Some(record) = state
        .node_async_hooks
        .resources
        .get_mut(&value::decode_handle(resource))
    {
        record.gc_destroy = false;
    }
    let Some(record) = state
        .node_async_hooks
        .resources
        .get(&value::decode_handle(resource))
    else {
        return Err(fail_dispatch(ctx));
    };
    Ok((
        resource,
        AsyncContextSnapshot {
            stores: record.stores.clone(),
            execution_async_id: record.async_id,
            trigger_async_id: record.trigger_async_id,
            resource,
        },
    ))
}

pub(crate) fn destroy_scheduled_resource(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    resource: i64,
) -> Option<i64> {
    let result = resource_destroy(ctx, state, &[resource]);
    value::is_exception(result).then_some(result)
}

pub(crate) fn collect_auto_resources(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
) -> Option<i64> {
    let resources = state
        .node_async_hooks
        .resources
        .values()
        .filter(|resource| resource.gc_destroy && !resource.destroyed)
        .map(|resource| resource.resource)
        .collect::<Vec<_>>();
    for resource in resources {
        let result = resource_destroy(ctx, state, &[resource]);
        if value::is_exception(result) {
            return Some(result);
        }
    }
    None
}

fn als_new(state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let key = state.node_async_hooks.next_als_key;
    state.node_async_hooks.next_als_key = key.saturating_add(1);
    let has_default = args
        .first()
        .is_some_and(|value| value::is_bool(*value) && value::decode_bool(*value));
    if has_default {
        state.node_async_hooks.defaults.insert(
            key,
            args.get(1).copied().unwrap_or_else(value::encode_undefined),
        );
    }
    value::encode_f64(f64::from(key))
}

fn als_get_store(state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(key) = args.first().and_then(|value| key(*value)) else {
        return value::encode_undefined();
    };
    state
        .node_async_hooks
        .current
        .stores
        .as_ref()
        .and_then(|stores| stores.get(&key))
        .or_else(|| state.node_async_hooks.defaults.get(&key))
        .copied()
        .unwrap_or_else(value::encode_undefined)
}

fn als_enter_with(state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(key) = args.first().and_then(|value| key(*value)) else {
        return value::encode_undefined();
    };
    let stored = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    state
        .node_async_hooks
        .current
        .stores
        .get_or_insert_with(HashMap::new)
        .insert(key, stored);
    value::encode_undefined()
}

fn als_disable(state: &mut NativeAgentState, args: &[i64]) -> i64 {
    if let Some(key) = args.first().and_then(|value| key(*value))
        && let Some(stores) = state.node_async_hooks.current.stores.as_mut()
    {
        stores.remove(&key);
    }
    value::encode_undefined()
}

fn capture_frame(state: &mut NativeAgentState) -> i64 {
    let Some(frame) = state.node_async_hooks.current.stores.clone() else {
        return value::encode_f64(-1.0);
    };
    let Ok(id) = u32::try_from(state.node_async_hooks.captured_frames.len()) else {
        return value::encode_f64(-1.0);
    };
    state.node_async_hooks.captured_frames.push(Some(frame));
    value::encode_f64(f64::from(id))
}

fn als_push_frame(state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let previous = capture_frame(state);
    state.node_async_hooks.current.stores = frame_from_argument(state, args.first().copied());
    previous
}

fn als_pop_frame(state: &mut NativeAgentState, args: &[i64]) -> i64 {
    state.node_async_hooks.current.stores = frame_from_argument(state, args.first().copied());
    value::encode_undefined()
}

fn frame_from_argument(
    state: &NativeAgentState,
    encoded: Option<i64>,
) -> Option<HashMap<u32, i64>> {
    let id = encoded
        .filter(|value| value::is_f64(*value))
        .map(value::decode_f64)?;
    if id < 0.0 {
        return None;
    }
    state
        .node_async_hooks
        .captured_frames
        .get(id as usize)
        .cloned()
        .flatten()
}

fn resource_init(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [resource, type_value] = args else {
        return fail_dispatch(ctx);
    };
    let Some(resource_handle) = super::object_handle(*resource) else {
        return fail_dispatch(ctx);
    };
    let Some(type_name) = state.string(*type_value) else {
        return type_error(ctx, state, "The type argument must be a string");
    };
    let type_name = type_name.to_utf8_lossy();
    let async_id = state.node_async_hooks.next_async_id;
    state.node_async_hooks.next_async_id = async_id.saturating_add(1);
    let trigger_async_id = state.node_async_hooks.current.execution_async_id;
    state.node_async_hooks.resources.insert(
        resource_handle,
        AsyncResourceRecord {
            async_id,
            trigger_async_id,
            resource: *resource,
            stores: state.node_async_hooks.current.stores.clone(),
            destroyed: false,
            promise: false,
            gc_destroy: true,
        },
    );
    let Some(type_value) = state.intern_text(type_name, value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    emit(
        ctx,
        state,
        HookPhase::Init,
        &[
            value::encode_f64(async_id as f64),
            type_value,
            value::encode_f64(trigger_async_id as f64),
            *resource,
        ],
        false,
    )
    .unwrap_or(*resource)
}

fn resource_new(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(type_name) = args.first().and_then(|value| state.string(*value)) else {
        return type_error(ctx, state, "The type argument must be a string");
    };
    let type_name = type_name.to_utf8_lossy();
    if type_name.is_empty()
        && state
            .node_async_hooks
            .hooks
            .iter()
            .any(|hook| hook.enabled && value::is_callable(hook.init))
    {
        return type_error(ctx, state, "Invalid type: empty string");
    }
    let trigger = args
        .get(1)
        .and_then(|options| resource_trigger_option(state, *options))
        .unwrap_or(state.node_async_hooks.current.execution_async_id);
    let require_manual_destroy = args
        .get(1)
        .and_then(|options| modules::named_property(state, *options, "requireManualDestroy"))
        .is_some_and(|manual| value::is_bool(manual) && value::decode_bool(manual));
    let async_id = state.node_async_hooks.next_async_id;
    state.node_async_hooks.next_async_id = async_id.saturating_add(1);
    let Ok(resource) = state.allocate_object(0, false) else {
        return fail_dispatch(ctx);
    };
    state.node_async_hooks.resources.insert(
        value::decode_handle(resource),
        AsyncResourceRecord {
            async_id,
            trigger_async_id: trigger,
            resource,
            stores: state.node_async_hooks.current.stores.clone(),
            destroyed: false,
            promise: false,
            gc_destroy: !require_manual_destroy,
        },
    );
    let Some(type_value) = state.intern_text(type_name, value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    emit(
        ctx,
        state,
        HookPhase::Init,
        &[
            value::encode_f64(async_id as f64),
            type_value,
            value::encode_f64(trigger as f64),
            resource,
        ],
        false,
    )
    .unwrap_or(resource)
}

fn resource_enter(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(resource) = args.first().copied() else {
        return value::encode_undefined();
    };
    let Some(record) = state
        .node_async_hooks
        .resources
        .get(&value::decode_handle(resource))
        .cloned()
    else {
        return value::encode_undefined();
    };
    state
        .node_async_hooks
        .execution_stack
        .push(state.node_async_hooks.current.clone());
    state.node_async_hooks.current = AsyncContextSnapshot {
        stores: record.stores,
        execution_async_id: record.async_id,
        trigger_async_id: record.trigger_async_id,
        resource: record.resource,
    };
    emit(
        ctx,
        state,
        HookPhase::Before,
        &[value::encode_f64(record.async_id as f64)],
        false,
    )
    .unwrap_or_else(value::encode_undefined)
}

fn resource_exit(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let async_id = args
        .first()
        .and_then(|resource| {
            state
                .node_async_hooks
                .resources
                .get(&value::decode_handle(*resource))
        })
        .map(|resource| resource.async_id);
    let result = async_id.and_then(|async_id| {
        emit(
            ctx,
            state,
            HookPhase::After,
            &[value::encode_f64(async_id as f64)],
            false,
        )
    });
    if let Some(previous) = state.node_async_hooks.execution_stack.pop() {
        state.node_async_hooks.current = previous;
    }
    result.unwrap_or_else(value::encode_undefined)
}

fn resource_destroy(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(resource) = args.first().copied() else {
        return value::encode_undefined();
    };
    let Some(record) = state
        .node_async_hooks
        .resources
        .get_mut(&value::decode_handle(resource))
    else {
        return resource;
    };
    if record.destroyed {
        return resource;
    }
    record.destroyed = true;
    let async_id = record.async_id;
    emit(
        ctx,
        state,
        HookPhase::Destroy,
        &[value::encode_f64(async_id as f64)],
        false,
    )
    .unwrap_or(resource)
}

fn resource_id(state: &NativeAgentState, args: &[i64], trigger: bool) -> i64 {
    args.first()
        .and_then(|resource| {
            state
                .node_async_hooks
                .resources
                .get(&value::decode_handle(*resource))
        })
        .map(|resource| {
            value::encode_f64(if trigger {
                resource.trigger_async_id as f64
            } else {
                resource.async_id as f64
            })
        })
        .unwrap_or_else(value::encode_undefined)
}

fn resource_trigger_option(state: &mut NativeAgentState, options: i64) -> Option<u64> {
    if value::is_f64(options) {
        return Some(value::decode_f64(options) as u64);
    }
    modules::named_property(state, options, "triggerAsyncId")
        .filter(|value| value::is_f64(*value))
        .map(value::decode_f64)
        .map(|value| value as u64)
}

fn create_hook(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let callback = |index: usize| {
        args.get(index)
            .copied()
            .filter(|callback| value::is_callable(*callback))
            .unwrap_or_else(value::encode_undefined)
    };
    let Ok(id) = u32::try_from(state.node_async_hooks.hooks.len()) else {
        return fail_dispatch(ctx);
    };
    state.node_async_hooks.hooks.push(HookRecord {
        init: callback(0),
        before: callback(1),
        after: callback(2),
        destroy: callback(3),
        promise_resolve: callback(4),
        track_promises: args
            .get(5)
            .is_some_and(|value| value::is_bool(*value) && value::decode_bool(*value)),
        enabled: false,
    });
    let Ok(object) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
    let Some(enable) = state.native_callable(NativeCallableKind::NodeAsyncHooks(
        NodeAsyncHooksCallable::HookEnable(id),
    )) else {
        return fail_dispatch(ctx);
    };
    let Some(disable) = state.native_callable(NativeCallableKind::NodeAsyncHooks(
        NodeAsyncHooksCallable::HookDisable(id),
    )) else {
        return fail_dispatch(ctx);
    };
    if modules::set_named_property(state, object, "enable", enable).is_err()
        || modules::set_named_property(state, object, "disable", disable).is_err()
    {
        return fail_dispatch(ctx);
    }
    object
}

fn set_hook_enabled(state: &mut NativeAgentState, id: u32, enabled: bool, object: i64) -> i64 {
    if let Some(hook) = state.node_async_hooks.hooks.get_mut(id as usize) {
        hook.enabled = enabled;
    }
    object
}

fn providers(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let Ok(object) = state.allocate_object(3, false) else {
        return fail_dispatch(ctx);
    };
    for (name, id) in [("NONE", 0.0), ("PROMISE", 27.0), ("ELDHISTOGRAM", 3.0)] {
        if modules::set_named_property(state, object, name, value::encode_f64(id)).is_err() {
            return fail_dispatch(ctx);
        }
    }
    object
}

#[derive(Clone, Copy)]
enum HookPhase {
    Init,
    Before,
    After,
    Destroy,
    PromiseResolve,
}

fn emit(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    phase: HookPhase,
    args: &[i64],
    promise: bool,
) -> Option<i64> {
    let callbacks = state
        .node_async_hooks
        .hooks
        .iter()
        .filter(|hook| hook.enabled && (!promise || hook.track_promises))
        .filter_map(|hook| {
            let callback = match phase {
                HookPhase::Init => hook.init,
                HookPhase::Before => hook.before,
                HookPhase::After => hook.after,
                HookPhase::Destroy => hook.destroy,
                HookPhase::PromiseResolve => hook.promise_resolve,
            };
            value::is_callable(callback).then_some(callback)
        })
        .collect::<Vec<_>>();
    for callback in callbacks {
        let result = state
            .invoke_callable(ctx, callback, value::encode_undefined(), args)
            .unwrap_or_else(|| fail_dispatch(ctx));
        if value::is_exception(result) {
            state.fatal_exception = Some(result);
            return Some(result);
        }
    }
    None
}

fn key(encoded: i64) -> Option<u32> {
    value::is_f64(encoded)
        .then(|| value::decode_f64(encoded))
        .filter(|value| value.is_finite() && *value >= 0.0 && value.fract() == 0.0)
        .map(|value| value as u32)
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    modules::named_error_object(state, "TypeError", message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
