use super::*;
use crate::wasm_env::WasmEnv;

pub(crate) trait RuntimeStateAccess {
    fn state_mut(&mut self) -> &mut RuntimeState;
}

impl RuntimeStateAccess for Caller<'_, RuntimeState> {
    fn state_mut(&mut self) -> &mut RuntimeState {
        Caller::data_mut(self)
    }
}

impl RuntimeStateAccess for Store<RuntimeState> {
    fn state_mut(&mut self) -> &mut RuntimeState {
        Store::data_mut(self)
    }
}

pub(crate) fn promise_entry_mut(
    table: &mut [PromiseEntry],
    handle: usize,
) -> Option<&mut PromiseEntry> {
    table.get_mut(handle).filter(|entry| entry.is_promise)
}

pub(crate) fn promise_entry(table: &[PromiseEntry], handle: usize) -> Option<&PromiseEntry> {
    table.get(handle).filter(|entry| entry.is_promise)
}

pub(crate) fn is_promise_value(state: &RuntimeState, val: i64) -> bool {
    if !value::is_object(val) {
        return false;
    }
    let handle = value::decode_object_handle(val) as usize;
    let table = state.promise_table.lock().expect("promise table mutex");
    promise_entry(&table, handle).is_some()
}

pub(crate) fn create_native_callable(state: &RuntimeState, callable: NativeCallable) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let mut free_slots = state
        .native_callable_free_slots
        .lock()
        .expect("native callable free slots mutex");
    let handle = if let Some(slot) = free_slots.pop() {
        table[slot as usize] = callable;
        slot
    } else {
        let handle = table.len() as u32;
        table.push(callable);
        handle
    };
    value::encode_native_callable_idx(handle)
}

/// 仅用于一次性内部 native handler 的槽位复用；JS 持有的 resolve/reject 槽位必须保持稳定。
pub(crate) fn recycle_native_callable(state: &RuntimeState, callable: i64) {
    if !value::is_native_callable(callable) {
        return;
    }
    let idx = value::decode_native_callable_idx(callable) as usize;
    let record = state
        .native_callables
        .lock()
        .expect("native callable table mutex")
        .get(idx)
        .cloned();
    if matches!(
        record,
        Some(NativeCallable::PromiseResolvingFunction { .. })
    ) {
        return;
    }
    state
        .native_callable_free_slots
        .lock()
        .expect("native callable free slots mutex")
        .push(idx as u32);
}

pub(crate) fn create_promise_resolving_function(
    state: &RuntimeState,
    promise: i64,
    already_resolved: Arc<Mutex<bool>>,
    kind: PromiseResolvingKind,
) -> i64 {
    create_native_callable(
        state,
        NativeCallable::PromiseResolvingFunction {
            promise,
            already_resolved,
            kind,
        },
    )
}

pub(crate) fn create_promise_resolving_functions(state: &RuntimeState, promise: i64) -> (i64, i64) {
    let already_resolved = Arc::new(Mutex::new(false));
    let resolve = create_promise_resolving_function(
        state,
        promise,
        Arc::clone(&already_resolved),
        PromiseResolvingKind::Fulfill,
    );
    let reject = create_promise_resolving_function(
        state,
        promise,
        already_resolved,
        PromiseResolvingKind::Reject,
    );
    (resolve, reject)
}

pub(crate) fn alloc_promise_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    entry: PromiseEntry,
) -> i64 {
    let promise = {
        let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &_wjsm_env, 0)
    };
    if value::is_object(promise) {
        let handle = value::decode_object_handle(promise) as usize;
        let mut table = caller
            .data()
            .promise_table
            .lock()
            .expect("promise table mutex");
        insert_promise_entry(&mut table, handle, entry);
    }
    promise
}

// ── §27.2.1.3 NewPromiseCapability(C) ─────────────────────────────────
/// 创建 PromiseCapability = { [[Promise]], [[Resolve]], [[Reject]] }。
/// 当 constructor 为 undefined/null 时使用内建 Promise 快速路径；
/// 否则记录构造器引用（用于 species-aware 操作）。
pub(crate) fn new_promise_capability_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    constructor: i64,
) -> (i64, i64, i64) {
    let mut entry = PromiseEntry::pending();
    // 如果构造器不是 undefined/null，记录到 entry 中用于后续 species 查找
    if !value::is_undefined(constructor) && !value::is_null(constructor) {
        entry.constructor_handle = Some(constructor);
    }
    let promise = alloc_promise_from_caller(caller, entry);
    let (resolve, reject) = create_promise_resolving_functions(caller.data(), promise);
    (promise, resolve, reject)
}

pub(crate) fn is_promise_settled(state: &RuntimeState, promise: i64) -> bool {
    let handle = raw_promise_handle(promise);
    let table = state.promise_table.lock().expect("promise table mutex");
    promise_entry(&table, handle)
        .map(|entry| !matches!(entry.state, PromiseState::Pending))
        .unwrap_or(true)
}

pub(crate) fn queue_promise_reactions(
    state: &RuntimeState,
    reactions: Vec<PromiseReaction>,
    value: i64,
    is_rejected: bool,
) {
    let mut queue = state.microtask_queue.lock().expect("microtask queue mutex");
    for reaction in reactions {
        match reaction.kind {
            PromiseReactionKind::AsyncResume {
                fn_table_idx,
                state: resume_state,
            } => {
                queue.push_back(Microtask::AsyncResume {
                    fn_table_idx,
                    continuation: reaction.target_promise,
                    state: resume_state,
                    resume_val: value,
                    is_rejected,
                });
            }
            PromiseReactionKind::Normal { handler } => {
                queue.push_back(Microtask::PromiseReaction {
                    promise: reaction.target_promise,
                    reaction_type: reaction.reaction_type,
                    handler,
                    argument: value,
                });
            }
        }
    }
}

pub(crate) fn settle_promise(state: &RuntimeState, promise: i64, settlement: PromiseSettlement) {
    let handle = raw_promise_handle(promise);
    let (reactions, value, is_rejected) = {
        let mut table = state.promise_table.lock().expect("promise table mutex");
        let Some(entry) = promise_entry_mut(&mut table, handle) else {
            return;
        };
        if !matches!(entry.state, PromiseState::Pending) {
            return;
        }
        match settlement {
            PromiseSettlement::Fulfill(value) => {
                let reactions = std::mem::take(&mut entry.fulfill_reactions);
                entry.state = PromiseState::Fulfilled(value);
                (reactions, value, false)
            }
            PromiseSettlement::Reject(reason) => {
                if !entry.handled {
                    state
                        .pending_unhandled_rejections
                        .lock()
                        .expect("pending_unhandled_rejections mutex")
                        .insert(handle);
                }
                let reactions = std::mem::take(&mut entry.reject_reactions);
                entry.state = PromiseState::Rejected(reason);
                (reactions, reason, true)
            }
        }
    };
    queue_promise_reactions(state, reactions, value, is_rejected);
}

pub(crate) fn adopt_promise(state: &RuntimeState, promise: i64, source: i64) {
    let target_handle = raw_promise_handle(promise);
    let source_handle = raw_promise_handle(source);
    let mut queued = None;
    {
        let mut table = state.promise_table.lock().expect("promise table mutex");
        let Some(source_entry) = promise_entry_mut(&mut table, source_handle) else {
            return;
        };
        source_entry.handled = true;
        clear_pending_unhandled_rejection(state, source_handle);
        match source_entry.state.clone() {
            PromiseState::Pending => {
                source_entry.fulfill_reactions.push(PromiseReaction::new(
                    value::encode_undefined(),
                    target_handle as i64,
                    ReactionType::Fulfill,
                ));
                source_entry.reject_reactions.push(PromiseReaction::new(
                    value::encode_undefined(),
                    target_handle as i64,
                    ReactionType::Reject,
                ));
            }
            PromiseState::Fulfilled(value) => {
                queued = Some((ReactionType::Fulfill, value));
            }
            PromiseState::Rejected(reason) => {
                queued = Some((ReactionType::Reject, reason));
            }
        }
    }
    if let Some((reaction_type, argument)) = queued {
        let mut queue = state.microtask_queue.lock().expect("microtask queue mutex");
        queue.push_back(Microtask::PromiseReaction {
            promise: target_handle as i64,
            reaction_type,
            handler: value::encode_undefined(),
            argument,
        });
    }
}

pub(crate) fn resolve_promise<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    promise: i64,
    resolution: i64,
) {
    if promise == resolution {
        let reason = runtime_error_value(
            ctx.state_mut(),
            "TypeError: cannot resolve promise with itself".to_string(),
        );
        settle_promise(ctx.state_mut(), promise, PromiseSettlement::Reject(reason));
        return;
    }

    if is_promise_value(ctx.state_mut(), resolution) {
        adopt_promise(ctx.state_mut(), promise, resolution);
        return;
    }

    if (value::is_object(resolution)
        || value::is_function(resolution)
        || value::is_callable(resolution))
        && let Some(ptr) =
            resolve_handle_idx_with_env(ctx, env, (resolution as u64 & 0xFFFF_FFFF) as usize)
        && let Some(then) = read_object_property_by_name_with_env(ctx, env, ptr, "then")
        && value::is_callable(then)
    {
        let mut queue = ctx
            .state_mut()
            .microtask_queue
            .lock()
            .expect("microtask queue mutex");
        queue.push_back(Microtask::PromiseResolveThenable {
            promise,
            thenable: resolution,
            then,
        });
        return;
    }

    settle_promise(
        ctx.state_mut(),
        promise,
        PromiseSettlement::Fulfill(resolution),
    );
}

#[inline]
pub(crate) fn resolve_promise_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    promise: i64,
    resolution: i64,
) {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    resolve_promise(caller, &env, promise, resolution);
}

pub(crate) fn passive_reaction_settlement(
    reaction_type: ReactionType,
    argument: i64,
) -> PromiseSettlement {
    match reaction_type {
        ReactionType::Fulfill | ReactionType::FinallyFulfill => {
            PromiseSettlement::Fulfill(argument)
        }
        ReactionType::Reject | ReactionType::FinallyReject => PromiseSettlement::Reject(argument),
    }
}

pub(crate) fn runtime_error_value(state: &RuntimeState, message: String) -> i64 {
    let mut table = state.runtime_strings.lock().expect("runtime strings mutex");
    let handle = table.len() as u32;
    table.push(message);
    value::encode_runtime_string_handle(handle)
}

pub(crate) fn set_runtime_error(state: &RuntimeState, message: String) {
    let mut error_lock = state.runtime_error.lock().expect("runtime_error mutex");
    if error_lock.is_none() {
        *error_lock = Some(message);
    }
}

pub(crate) fn nanbox_to_usize(val: i64) -> usize {
    if value::is_bool(val) {
        if value::decode_bool(val) { 1 } else { 0 }
    } else {
        f64::from_bits(val as u64) as usize
    }
}

pub(crate) fn nanbox_to_u32(val: i64) -> u32 {
    nanbox_to_usize(val) as u32
}

pub(crate) fn nanbox_to_bool(val: i64) -> bool {
    if value::is_bool(val) {
        value::decode_bool(val)
    } else {
        f64::from_bits(val as u64) != 0.0
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    /// 第二次 resolve 不得覆盖 PromiseResolvingFunction 槽位（recycle 已禁用）。
    #[test]
    fn resolver_double_call_keeps_slot_and_noops() {
        let state = RuntimeState::new();
        let p = value::encode_f64(99.0);
        let (resolve, _reject) = create_promise_resolving_functions(&state, p);
        let idx1 = value::decode_native_callable_idx(resolve);
        recycle_native_callable(&state, resolve);
        let idx2 = value::decode_native_callable_idx(resolve);
        assert_eq!(idx1, idx2);
        let record = state
            .native_callables
            .lock()
            .expect("native callable table mutex")
            .get(idx1 as usize)
            .cloned();
        assert!(matches!(
            record,
            Some(NativeCallable::PromiseResolvingFunction { .. })
        ));
    }
}
