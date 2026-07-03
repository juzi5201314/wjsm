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
    let table = state
        .promise_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    promise_entry(&table, handle).is_some()
}

pub(crate) fn create_native_callable(state: &RuntimeState, callable: NativeCallable) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut free_slots = state
        .native_callable_free_slots
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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
        .unwrap_or_else(|e| e.into_inner())
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
        .unwrap_or_else(|e| e.into_inner())
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
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let promise = alloc_host_object(caller, &env, 0);
    if value::is_object(promise) {
        if !value::is_object(caller.data().promise_prototype) {
            crate::runtime_heap::ensure_promise_prototype_initialized(caller, &env);
        }
        let proto = caller.data().promise_prototype;
        if value::is_object(proto) {
            crate::runtime_heap::set_object_proto_header(caller, &env, promise, proto);
        }
        let handle = value::decode_object_handle(promise) as usize;
        let mut table = caller
            .data()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        insert_promise_entry(&mut table, handle, entry);
    }
    promise
}

/// 内建 %Promise% 构造器（`NativeCallable::PromiseConstructor`）。
fn default_promise_constructor(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    create_native_callable(caller.data(), NativeCallable::PromiseConstructor)
}

fn is_native_promise_constructor(caller: &mut Caller<'_, RuntimeState>, constructor: i64) -> bool {
    if !value::is_native_callable(constructor) {
        return false;
    }
    let idx = value::decode_native_callable_idx(constructor) as usize;
    let table = caller
        .data()
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    matches!(table.get(idx), Some(NativeCallable::PromiseConstructor))
}

/// ES2024 `SpeciesConstructor(O, %Promise%)`：读取 `O.constructor[Symbol.species]`。
pub(crate) fn promise_species_constructor(
    caller: &mut Caller<'_, RuntimeState>,
    exemplar: i64,
) -> i64 {
    let default_ctor = default_promise_constructor(caller);
    if !value::is_js_object(exemplar) {
        return default_ctor;
    }
    let Some(exemplar_ptr) = resolve_handle(caller, exemplar) else {
        return default_ctor;
    };
    let mut constructor = read_object_property_by_name(caller, exemplar_ptr, "constructor")
        .unwrap_or_else(value::encode_undefined);
    if value::is_undefined(constructor) && is_promise_value(caller.data(), exemplar) {
        let handle = raw_promise_handle(exemplar);
        let table = caller
            .data()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        constructor = table
            .get(handle)
            .and_then(|entry| entry.constructor_handle)
            .unwrap_or_else(value::encode_undefined);
    }
    if is_native_promise_constructor(caller, constructor) {
        return default_ctor;
    }
    if value::is_js_object(constructor) {
        let species = get_by_name_id_sync(
            caller,
            constructor,
            encode_symbol_name_id(wjsm_ir::wk_symbol::SPECIES),
        );
        if value::is_null(species) {
            constructor = value::encode_undefined();
        } else if !value::is_undefined(species) {
            constructor = species;
        }
    }
    if value::is_undefined(constructor) {
        default_ctor
    } else {
        constructor
    }
}

/// `.then()` / `.catch()` / `.finally()` 结果 promise 的 `constructor_handle`（`None` = 内建 Promise）。
pub(crate) fn promise_result_species_constructor_handle(
    caller: &mut Caller<'_, RuntimeState>,
    exemplar: i64,
) -> Option<i64> {
    let species = promise_species_constructor(caller, exemplar);
    if is_native_promise_constructor(caller, species) {
        None
    } else {
        Some(species)
    }
}

pub(crate) fn set_promise_proto_from_constructor(
    caller: &mut Caller<'_, RuntimeState>,
    promise: i64,
    constructor: Option<i64>,
) {
    let proto_from_constructor = constructor.and_then(|ctor| {
        if is_native_promise_constructor(caller, ctor) {
            return None;
        }
        let ctor_ptr = resolve_handle(caller, ctor)?;
        let proto = read_object_property_by_name(caller, ctor_ptr, "prototype")
            .unwrap_or_else(value::encode_undefined);
        value::is_js_object(proto).then_some(proto)
    });
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    if !value::is_object(caller.data().promise_prototype) {
        crate::runtime_heap::ensure_promise_prototype_initialized(caller, &env);
    }
    let proto = proto_from_constructor.unwrap_or(caller.data().promise_prototype);
    if value::is_js_object(proto) {
        crate::runtime_heap::set_object_proto_header(caller, &env, promise, proto);
    }
}

// ── §27.2.1.3 NewPromiseCapability(C) ─────────────────────────────────
/// 创建 PromiseCapability = { [[Promise]], [[Resolve]], [[Reject]] }。
/// 当 constructor 为 undefined/null 时使用内建 Promise 快速路径；
/// 否则记录构造器引用（用于 species-aware 操作）。
pub(crate) fn new_promise_capability_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    constructor: i64,
) -> (i64, i64, i64) {
    let constructor_handle =
        (!value::is_undefined(constructor) && !value::is_null(constructor)).then_some(constructor);
    let mut entry = PromiseEntry::pending();
    entry.constructor_handle = constructor_handle;
    let promise = alloc_promise_from_caller(caller, entry);
    set_promise_proto_from_constructor(caller, promise, constructor_handle);
    let (resolve, reject) = create_promise_resolving_functions(caller.data(), promise);
    (promise, resolve, reject)
}

pub(crate) fn is_promise_settled(state: &RuntimeState, promise: i64) -> bool {
    let handle = raw_promise_handle(promise);
    let table = state
        .promise_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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
    let mut queue = state
        .microtask_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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
                    completion: if is_rejected { 1 } else { 0 },
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
        let mut table = state
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
                        .unwrap_or_else(|e| e.into_inner())
                        .push_back(handle);
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
        let mut table = state
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
        let mut queue = state
            .microtask_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
            .unwrap_or_else(|e| e.into_inner());
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

/// 判断 value 是否为 thenable（native promise，或带可调用 `then` 的对象/函数）。
/// 与 `resolve_promise` 的 thenable 探测逻辑保持一致。
pub(crate) fn is_thenable_value<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    value: i64,
) -> bool {
    if is_promise_value(ctx.state_mut(), value) {
        return true;
    }
    if !(value::is_object(value) || value::is_function(value) || value::is_callable(value)) {
        return false;
    }
    let Some(ptr) = resolve_handle_idx_with_env(ctx, env, (value as u64 & 0xFFFF_FFFF) as usize)
    else {
        return false;
    };
    match read_object_property_by_name_with_env(ctx, env, ptr, "then") {
        Some(then) => value::is_callable(then),
        None => false,
    }
}

/// 从泛型 ctx 分配一个 promise（`alloc_promise` 的 ctx+env 版本）。
pub(crate) fn alloc_promise_with_env<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    entry: PromiseEntry,
) -> i64 {
    let promise = alloc_object_with_env(ctx, env, 0);
    if value::is_object(promise) {
        if !value::is_object(ctx.as_context().data().promise_prototype) {
            crate::runtime_heap::ensure_promise_prototype_initialized(ctx, env);
        }
        let proto = ctx.as_context().data().promise_prototype;
        if value::is_object(proto) {
            crate::runtime_heap::set_object_proto_header(ctx, env, promise, proto);
        }
        let handle = value::decode_object_handle(promise) as usize;
        let mut table = ctx
            .state_mut()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        insert_promise_entry(&mut table, handle, entry);
    }
    promise
}

/// §27.2.5.4.1/2 ThenFinally/CatchFinally：处理 onFinally 的返回值 `result`。
/// 非 thenable → 直接按原始结算值结算 target；thenable → 创建中间 promise adopt
/// 其状态，并挂上 `PromiseFinallyAwait` 反应，待其 settle 后再结算 target。
pub(crate) fn settle_finally_reaction<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    target_promise: i64,
    original_value: i64,
    result: i64,
    reaction_type: ReactionType,
) {
    let finally_is_reject = matches!(reaction_type, ReactionType::FinallyReject);
    // onFinally 自身抛异常 → result promise 以抛出值 reject（abrupt completion 覆盖原结果）。
    if value::is_exception(result) {
        let reason = exception_reason_from_state(ctx.state_mut(), result);
        settle_promise(
            ctx.state_mut(),
            target_promise,
            PromiseSettlement::Reject(reason),
        );
        return;
    }
    if !is_thenable_value(ctx, env, result) {
        let settlement = if finally_is_reject {
            PromiseSettlement::Reject(original_value)
        } else {
            PromiseSettlement::Fulfill(original_value)
        };
        settle_promise(ctx.state_mut(), target_promise, settlement);
        return;
    }
    let inner = alloc_promise_with_env(ctx, env, PromiseEntry::pending());
    let handler = create_native_callable(
        ctx.state_mut(),
        NativeCallable::PromiseFinallyAwait {
            target_promise,
            original_value,
            finally_is_reject,
        },
    );
    {
        let inner_handle = raw_promise_handle(inner);
        let mut table = ctx
            .state_mut()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = promise_entry_mut(&mut table, inner_handle) {
            entry.handled = true;
            entry.fulfill_reactions.push(PromiseReaction::new(
                handler,
                target_promise,
                ReactionType::Fulfill,
            ));
            entry.reject_reactions.push(PromiseReaction::new(
                handler,
                target_promise,
                ReactionType::Reject,
            ));
        }
    }
    resolve_promise(ctx, env, inner, result);
}

/// 拦截 `PromiseFinallyAwait` 反应（在 drain loop 中先于通用 callable 派发）。
/// 返回 true 表示已处理。
pub(crate) fn handle_finally_await_reaction<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    handler: i64,
    argument: i64,
    reaction_type: ReactionType,
) -> bool {
    let record = {
        if !value::is_native_callable(handler) {
            None
        } else {
            let idx = value::decode_native_callable_idx(handler) as usize;
            ctx.state_mut()
                .native_callables
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(idx)
                .and_then(|nc| match nc {
                    NativeCallable::PromiseFinallyAwait {
                        target_promise,
                        original_value,
                        finally_is_reject,
                    } => Some((*target_promise, *original_value, *finally_is_reject)),
                    _ => None,
                })
        }
    };
    let Some((target_promise, original_value, finally_is_reject)) = record else {
        return false;
    };
    recycle_native_callable(ctx.state_mut(), handler);
    let settlement = match reaction_type {
        // inner promise fulfilled → 按 finally 语义用原始值结算 target
        ReactionType::Fulfill | ReactionType::FinallyFulfill => {
            if finally_is_reject {
                PromiseSettlement::Reject(original_value)
            } else {
                PromiseSettlement::Fulfill(original_value)
            }
        }
        // inner promise rejected → 用 inner 的 reason reject target
        ReactionType::Reject | ReactionType::FinallyReject => PromiseSettlement::Reject(argument),
    };
    settle_promise(ctx.state_mut(), target_promise, settlement);
    true
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
    let mut table = state
        .runtime_strings
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(message.into());
    value::encode_runtime_string_handle(handle)
}

pub(crate) fn set_runtime_error(state: &RuntimeState, message: String) {
    let mut error_lock = state
        .runtime_error
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if error_lock.is_none() {
        *error_lock = Some(message);
    }
}

pub(crate) fn nanbox_to_usize(val: i64) -> usize {
    if value::is_bool(val) {
        if value::decode_bool(val) { 1 } else { 0 }
    } else {
        value::decode_f64(val) as usize
    }
}

pub(crate) fn nanbox_to_u32(val: i64) -> u32 {
    nanbox_to_usize(val) as u32
}

pub(crate) fn nanbox_to_bool(val: i64) -> bool {
    if value::is_bool(val) {
        value::decode_bool(val)
    } else {
        value::decode_f64(val) != 0.0
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
            .unwrap_or_else(|e| e.into_inner())
            .get(idx1 as usize)
            .cloned();
        assert!(matches!(
            record,
            Some(NativeCallable::PromiseResolvingFunction { .. })
        ));
    }
}
