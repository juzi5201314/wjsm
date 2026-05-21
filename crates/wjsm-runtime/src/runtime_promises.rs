use super::*;

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

pub(crate) fn create_promise_resolving_function(
    state: &RuntimeState,
    promise: i64,
    already_resolved: Arc<Mutex<bool>>,
    kind: PromiseResolvingKind,
) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::PromiseResolvingFunction {
        promise,
        already_resolved,
        kind,
    });
    value::encode_native_callable_idx(handle)
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
    let promise = alloc_host_object_from_caller(caller, 0);
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

pub(crate) fn create_async_generator_method(
    state: &RuntimeState,
    generator: i64,
    kind: AsyncGeneratorCompletionType,
) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::AsyncGeneratorMethod { generator, kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn alloc_iterator_result_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
    done: bool,
) -> i64 {
    let obj = alloc_host_object_from_caller(caller, 2);
    let _ = define_host_data_property_from_caller(caller, obj, "value", val);
    let _ = define_host_data_property_from_caller(caller, obj, "done", value::encode_bool(done));
    obj
}

pub(crate) fn enqueue_async_resume_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    continuation: i64,
    state: u32,
    resume_val: i64,
    is_rejected: bool,
) {
    let cont_handle = value::decode_object_handle(continuation) as usize;
    let fn_table_idx = {
        let mut table = caller
            .data()
            .continuation_table
            .lock()
            .expect("continuation table mutex");
        let Some(entry) = table.get_mut(cont_handle) else {
            return;
        };
        while entry.captured_vars.len() < 2 {
            entry.captured_vars.push(value::encode_undefined());
        }
        entry.captured_vars[0] = value::encode_f64(state as f64);
        entry.captured_vars[1] = value::encode_bool(is_rejected);
        entry.fn_table_idx
    };
    caller
        .data()
        .microtask_queue
        .lock()
        .expect("microtask queue mutex")
        .push_back(Microtask::AsyncResume {
            fn_table_idx,
            continuation,
            state,
            resume_val,
            is_rejected,
        });
}

pub(crate) enum AsyncGeneratorPumpAction {
    Resume {
        continuation: i64,
        state: u32,
        value: i64,
        is_rejected: bool,
    },
    SettleResumePromise {
        promise: i64,
        value: i64,
        is_rejected: bool,
    },
    Fulfill {
        promise: i64,
        value: i64,
        done: bool,
    },
    Reject {
        promise: i64,
        reason: i64,
    },
}

pub(crate) fn pump_async_generator_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    generator: i64,
) {
    let handle = value::decode_object_handle(generator) as usize;
    let action = {
        let mut table = caller
            .data()
            .async_generator_table
            .lock()
            .expect("async generator table mutex");
        let Some(entry) = table.get_mut(handle) else {
            return;
        };
        match entry.state {
            AsyncGeneratorState::Executing | AsyncGeneratorState::Completed => None,
            AsyncGeneratorState::SuspendedYield => {
                let Some(resume_promise) = entry.waiting_resume_promise.take() else {
                    return;
                };
                if entry.queue.is_empty() {
                    entry.waiting_resume_promise = Some(resume_promise);
                    None
                } else {
                    let request = entry.queue.remove(0);
                    entry.active_request = Some(request);
                    entry.state = AsyncGeneratorState::Executing;
                    match request.completion_type {
                        AsyncGeneratorCompletionType::Next => {
                            Some(AsyncGeneratorPumpAction::SettleResumePromise {
                                promise: resume_promise,
                                value: request.value,
                                is_rejected: false,
                            })
                        }
                        AsyncGeneratorCompletionType::Throw => {
                            Some(AsyncGeneratorPumpAction::SettleResumePromise {
                                promise: resume_promise,
                                value: request.value,
                                is_rejected: true,
                            })
                        }
                        AsyncGeneratorCompletionType::Return => {
                            Some(AsyncGeneratorPumpAction::Fulfill {
                                promise: request.promise,
                                value: request.value,
                                done: true,
                            })
                        }
                    }
                }
            }
            AsyncGeneratorState::SuspendedStart => {
                if entry.queue.is_empty() {
                    None
                } else {
                    let request = entry.queue.remove(0);
                    match request.completion_type {
                        AsyncGeneratorCompletionType::Next => {
                            entry.active_request = Some(request);
                            entry.state = AsyncGeneratorState::Executing;
                            Some(AsyncGeneratorPumpAction::Resume {
                                continuation: entry.continuation,
                                state: 0,
                                value: request.value,
                                is_rejected: false,
                            })
                        }
                        AsyncGeneratorCompletionType::Return => {
                            entry.state = AsyncGeneratorState::Completed;
                            Some(AsyncGeneratorPumpAction::Fulfill {
                                promise: request.promise,
                                value: request.value,
                                done: true,
                            })
                        }
                        AsyncGeneratorCompletionType::Throw => {
                            entry.state = AsyncGeneratorState::Completed;
                            Some(AsyncGeneratorPumpAction::Reject {
                                promise: request.promise,
                                reason: request.value,
                            })
                        }
                    }
                }
            }
        }
    };
    match action {
        Some(AsyncGeneratorPumpAction::Resume {
            continuation,
            state,
            value,
            is_rejected,
        }) => enqueue_async_resume_from_caller(caller, continuation, state, value, is_rejected),
        Some(AsyncGeneratorPumpAction::SettleResumePromise {
            promise,
            value,
            is_rejected,
        }) => {
            if is_rejected {
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(value));
            } else {
                resolve_promise_from_caller(caller, promise, value);
            }
        }
        Some(AsyncGeneratorPumpAction::Fulfill {
            promise,
            value,
            done,
        }) => {
            let result = alloc_iterator_result_from_caller(caller, value, done);
            resolve_promise_from_caller(caller, promise, result);
        }
        Some(AsyncGeneratorPumpAction::Reject { promise, reason }) => {
            settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
        }
        None => {}
    }
}

pub(crate) fn create_combinator_context(
    state: &RuntimeState,
    result_promise: i64,
    result_array: i64,
) -> usize {
    let mut contexts = state
        .combinator_contexts
        .lock()
        .expect("combinator context mutex");
    let idx = contexts.len();
    contexts.push(CombinatorContext {
        result_promise,
        result_array,
        remaining: 0,
        settled: false,
    });
    idx
}

pub(crate) fn set_combinator_remaining(state: &RuntimeState, context: usize, remaining: usize) {
    if let Some(entry) = state
        .combinator_contexts
        .lock()
        .expect("combinator context mutex")
        .get_mut(context)
    {
        entry.remaining = remaining;
    }
}

pub(crate) fn mark_combinator_settled(state: &RuntimeState, context: usize) {
    if let Some(entry) = state
        .combinator_contexts
        .lock()
        .expect("combinator context mutex")
        .get_mut(context)
    {
        entry.settled = true;
    }
}

pub(crate) fn create_combinator_reaction_handler(
    state: &RuntimeState,
    context: usize,
    index: usize,
    kind: PromiseCombinatorReactionKind,
) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::PromiseCombinatorReaction {
        context,
        index,
        kind,
    });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn combinator_reaction_record(
    state: &RuntimeState,
    handler: i64,
) -> Option<(usize, usize, PromiseCombinatorReactionKind)> {
    if !value::is_native_callable(handler) {
        return None;
    }
    let idx = value::decode_native_callable_idx(handler) as usize;
    let record = state
        .native_callables
        .lock()
        .expect("native callable table mutex")
        .get(idx)
        .cloned()?;
    match record {
        NativeCallable::PromiseCombinatorReaction {
            context,
            index,
            kind,
        } => Some((context, index, kind)),
        _ => None,
    }
}

pub(crate) fn open_combinator_context(state: &RuntimeState, context: usize) -> Option<(i64, i64)> {
    let contexts = state
        .combinator_contexts
        .lock()
        .expect("combinator context mutex");
    let entry = contexts.get(context)?;
    if entry.settled {
        None
    } else {
        Some((entry.result_promise, entry.result_array))
    }
}

pub(crate) fn decrement_combinator_remaining(
    state: &RuntimeState,
    context: usize,
) -> Option<(i64, i64)> {
    let mut contexts = state
        .combinator_contexts
        .lock()
        .expect("combinator context mutex");
    let entry = contexts.get_mut(context)?;
    if entry.settled {
        return None;
    }
    entry.remaining = entry.remaining.saturating_sub(1);
    if entry.remaining == 0 {
        entry.settled = true;
        Some((entry.result_promise, entry.result_array))
    } else {
        None
    }
}

pub(crate) fn handle_combinator_reaction_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    handler: i64,
    argument: i64,
) -> bool {
    let Some((context, index, kind)) = combinator_reaction_record(caller.data(), handler) else {
        return false;
    };
    let Some((_, result_array)) = open_combinator_context(caller.data(), context) else {
        return true;
    };

    match kind {
        PromiseCombinatorReactionKind::AllFulfill => {
            if let Some(result_ptr) = resolve_array_ptr(caller, result_array) {
                write_array_elem(caller, result_ptr, index as u32, argument);
            }
            if let Some((result_promise, result_array)) =
                decrement_combinator_remaining(caller.data(), context)
            {
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Fulfill(result_array),
                );
            }
        }
        PromiseCombinatorReactionKind::AllSettledFulfill
        | PromiseCombinatorReactionKind::AllSettledReject => {
            let (status, value_name) = match kind {
                PromiseCombinatorReactionKind::AllSettledFulfill => ("fulfilled", "value"),
                PromiseCombinatorReactionKind::AllSettledReject => ("rejected", "reason"),
                _ => unreachable!(),
            };
            let record = alloc_all_settled_result_from_caller(caller, status, value_name, argument);
            if let Some(result_ptr) = resolve_array_ptr(caller, result_array) {
                write_array_elem(caller, result_ptr, index as u32, record);
            }
            if let Some((result_promise, result_array)) =
                decrement_combinator_remaining(caller.data(), context)
            {
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Fulfill(result_array),
                );
            }
        }
        PromiseCombinatorReactionKind::AnyReject => {
            if let Some(errors_ptr) = resolve_array_ptr(caller, result_array) {
                write_array_elem(caller, errors_ptr, index as u32, argument);
            }
            if let Some((result_promise, errors_array)) =
                decrement_combinator_remaining(caller.data(), context)
            {
                let aggregate = alloc_aggregate_error_from_caller(caller, errors_array);
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Reject(aggregate),
                );
            }
        }
    }
    true
}

pub(crate) fn handle_combinator_reaction_from_store(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    heap_ptr_global: &Global,
    obj_table_ptr_global: &Global,
    obj_table_count_global: &Global,
    handler: i64,
    argument: i64,
) -> bool {
    let Some((context, index, kind)) = combinator_reaction_record(store.data(), handler) else {
        return false;
    };
    let Some((_, result_array)) = open_combinator_context(store.data(), context) else {
        return true;
    };

    match kind {
        PromiseCombinatorReactionKind::AllFulfill => {
            if let Some(result_ptr) =
                resolve_handle_from_store(store, memory, obj_table_ptr_global, result_array)
            {
                write_array_elem_from_store(store, memory, result_ptr, index as u32, argument);
            }
            if let Some((result_promise, result_array)) =
                decrement_combinator_remaining(store.data(), context)
            {
                settle_promise(
                    store.data(),
                    result_promise,
                    PromiseSettlement::Fulfill(result_array),
                );
            }
        }
        PromiseCombinatorReactionKind::AllSettledFulfill
        | PromiseCombinatorReactionKind::AllSettledReject => {
            let (status, value_name) = match kind {
                PromiseCombinatorReactionKind::AllSettledFulfill => ("fulfilled", "value"),
                PromiseCombinatorReactionKind::AllSettledReject => ("rejected", "reason"),
                _ => unreachable!(),
            };
            let record = alloc_all_settled_result_from_store(
                store,
                memory,
                heap_ptr_global,
                obj_table_ptr_global,
                obj_table_count_global,
                status,
                value_name,
                argument,
            );
            if let Some(result_ptr) =
                resolve_handle_from_store(store, memory, obj_table_ptr_global, result_array)
            {
                write_array_elem_from_store(store, memory, result_ptr, index as u32, record);
            }
            if let Some((result_promise, result_array)) =
                decrement_combinator_remaining(store.data(), context)
            {
                settle_promise(
                    store.data(),
                    result_promise,
                    PromiseSettlement::Fulfill(result_array),
                );
            }
        }
        PromiseCombinatorReactionKind::AnyReject => {
            if let Some(errors_ptr) =
                resolve_handle_from_store(store, memory, obj_table_ptr_global, result_array)
            {
                write_array_elem_from_store(store, memory, errors_ptr, index as u32, argument);
            }
            if let Some((result_promise, errors_array)) =
                decrement_combinator_remaining(store.data(), context)
            {
                let aggregate = alloc_aggregate_error_from_store(
                    store,
                    memory,
                    heap_ptr_global,
                    obj_table_ptr_global,
                    obj_table_count_global,
                    errors_array,
                );
                settle_promise(
                    store.data(),
                    result_promise,
                    PromiseSettlement::Reject(aggregate),
                );
            }
        }
    }
    true
}

pub(crate) fn queue_promise_reactions(
    state: &RuntimeState,
    reactions: Vec<PromiseReaction>,
    value: i64,
    is_rejected: bool,
) {
    let mut queue = state.microtask_queue.lock().expect("microtask queue mutex");
    for reaction in reactions {
        if let Some(async_state) = reaction.async_resume_state {
            queue.push_back(Microtask::AsyncResume {
                fn_table_idx: reaction.handler as u32,
                continuation: reaction.target_promise,
                state: async_state as u32,
                resume_val: value,
                is_rejected,
            });
        } else {
            queue.push_back(Microtask::PromiseReaction {
                promise: reaction.target_promise,
                reaction_type: reaction.reaction_type,
                handler: reaction.handler,
                argument: value,
            });
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

pub(crate) fn resolve_promise_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    promise: i64,
    resolution: i64,
) {
    if promise == resolution {
        let reason = runtime_error_value(
            caller.data(),
            "TypeError: cannot resolve promise with itself".to_string(),
        );
        settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
        return;
    }

    if is_promise_value(caller.data(), resolution) {
        adopt_promise(caller.data(), promise, resolution);
        return;
    }

    if (value::is_object(resolution)
        || value::is_function(resolution)
        || value::is_callable(resolution))
        && let Some(ptr) = resolve_handle(caller, resolution)
        && let Some(then) = read_object_property_by_name(caller, ptr, "then")
        && value::is_callable(then)
    {
        let mut queue = caller
            .data()
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
        caller.data(),
        promise,
        PromiseSettlement::Fulfill(resolution),
    );
}

pub(crate) fn resolve_promise_from_store(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    obj_table_ptr_global: &Global,
    promise: i64,
    resolution: i64,
) {
    if promise == resolution {
        let reason = runtime_error_value(
            store.data(),
            "TypeError: cannot resolve promise with itself".to_string(),
        );
        settle_promise(store.data(), promise, PromiseSettlement::Reject(reason));
        return;
    }

    if is_promise_value(store.data(), resolution) {
        adopt_promise(store.data(), promise, resolution);
        return;
    }

    if (value::is_object(resolution)
        || value::is_function(resolution)
        || value::is_callable(resolution))
        && let Some(ptr) =
            resolve_handle_from_store(store, memory, obj_table_ptr_global, resolution)
        && let Some(then) = read_object_property_by_name_from_store(store, memory, ptr, "then")
        && value::is_callable(then)
    {
        let mut queue = store
            .data()
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
        store.data(),
        promise,
        PromiseSettlement::Fulfill(resolution),
    );
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

pub(crate) fn drain_microtasks_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    func_table: &Table,
) {
    loop {
        let task = {
            let mut queue = caller
                .data()
                .microtask_queue
                .lock()
                .expect("microtask queue mutex");
            queue.pop_front()
        };
        match task {
            Some(Microtask::PromiseReaction {
                promise,
                reaction_type,
                handler,
                argument,
            }) => {
                if handle_combinator_reaction_from_caller(caller, handler, argument) {
                    continue;
                }
                if value::is_callable(handler) {
                    // §27.2.5.3 — finally 的 handler 应以零参数调用（不传 value/reason）
                    let call_arg = match reaction_type {
                        ReactionType::FinallyFulfill | ReactionType::FinallyReject => {
                            value::encode_undefined()
                        }
                        _ => argument,
                    };
                    match call_host_function_from_caller(caller, func_table, handler, call_arg) {
                        Some(result) => match reaction_type {
                            ReactionType::Fulfill | ReactionType::Reject => {
                                resolve_promise_from_caller(caller, promise, result);
                            }
                            ReactionType::FinallyFulfill => {
                                settle_promise(
                                    caller.data(),
                                    promise,
                                    PromiseSettlement::Fulfill(argument),
                                );
                            }
                            ReactionType::FinallyReject => {
                                settle_promise(
                                    caller.data(),
                                    promise,
                                    PromiseSettlement::Reject(argument),
                                );
                            }
                        },
                        None => settle_promise(
                            caller.data(),
                            promise,
                            PromiseSettlement::Reject(runtime_error_value(
                                caller.data(),
                                "TypeError: promise reaction handler failed".to_string(),
                            )),
                        ),
                    }
                } else {
                    let settlement = passive_reaction_settlement(reaction_type, argument);
                    settle_promise(caller.data(), promise, settlement);
                }
            }
            Some(Microtask::PromiseResolveThenable {
                promise,
                thenable,
                then,
            }) => {
                let (resolve, reject) = create_promise_resolving_functions(caller.data(), promise);
                if call_host_function_from_caller(caller, func_table, then, resolve).is_none() {
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(reject));
                }
                let _ = thenable;
            }
            Some(Microtask::MicrotaskCallback { callback }) => {
                if value::is_callable(callback) {
                    let _ = call_host_function_from_caller(
                        caller,
                        func_table,
                        callback,
                        value::encode_undefined(),
                    );
                }
            }
            Some(Microtask::AsyncResume {
                fn_table_idx,
                continuation,
                state,
                resume_val,
                is_rejected,
            }) => {
                resume_async_function_from_caller(
                    caller,
                    func_table,
                    fn_table_idx,
                    continuation,
                    state,
                    resume_val,
                    is_rejected,
                );
            }
            Some(Microtask::CleanupFinalizationRegistry { callback, held_value: _ }) => {
                let _ = call_host_function_from_caller(
                    caller,
                    func_table,
                    callback,
                    value::encode_undefined(),
                );
            }
            None => break,
        }
    }
    // ── §27.2.1.9 HostPromiseRejectionTracker ────────────────────────────
    // 微任务队列排空后，扫描 promise table 检测未处理的 rejection 并输出警告
    let unhandled: Vec<i64> = {
        let table = caller
            .data()
            .promise_table
            .lock()
            .expect("promise table mutex");
        table
            .iter()
            .filter(|e| e.is_promise && !e.handled)
            .filter_map(|e| match &e.state {
                PromiseState::Rejected(reason) => Some(*reason),
                _ => None,
            })
            .collect()
    };
    for reason in unhandled {
        let msg = render_value(caller, reason).unwrap_or_else(|_| String::from("unknown"));
        eprintln!("UnhandledPromiseRejectionWarning: {msg}");
    }
}

pub(crate) fn drain_microtasks_from_store(
    store: &mut Store<RuntimeState>,
    func_table: &Table,
    memory: &Memory,
    shadow_sp_global: &Global,
    heap_ptr_global: &Global,
    obj_table_ptr_global: &Global,
    obj_table_count_global: &Global,
) {
    loop {
        let task = {
            let mut queue = store
                .data()
                .microtask_queue
                .lock()
                .expect("microtask queue mutex");
            queue.pop_front()
        };
        match task {
            Some(Microtask::PromiseReaction {
                promise,
                reaction_type,
                handler,
                argument,
            }) => {
                if handle_combinator_reaction_from_store(
                    store,
                    memory,
                    heap_ptr_global,
                    obj_table_ptr_global,
                    obj_table_count_global,
                    handler,
                    argument,
                ) {
                    continue;
                }
                if value::is_callable(handler) {
                    // §27.2.5.3 — finally 的 handler 应以零参数调用（不传 value/reason）
                    let call_arg = match reaction_type {
                        ReactionType::FinallyFulfill | ReactionType::FinallyReject => {
                            value::encode_undefined()
                        }
                        _ => argument,
                    };
                    match call_host_function_from_store(
                        store,
                        func_table,
                        memory,
                        shadow_sp_global,
                        handler,
                        call_arg,
                    ) {
                        Some(result) => match reaction_type {
                            ReactionType::Fulfill | ReactionType::Reject => {
                                resolve_promise_from_store(
                                    store,
                                    memory,
                                    obj_table_ptr_global,
                                    promise,
                                    result,
                                );
                            }
                            ReactionType::FinallyFulfill => {
                                settle_promise(
                                    store.data(),
                                    promise,
                                    PromiseSettlement::Fulfill(argument),
                                );
                            }
                            ReactionType::FinallyReject => {
                                settle_promise(
                                    store.data(),
                                    promise,
                                    PromiseSettlement::Reject(argument),
                                );
                            }
                        },
                        None => settle_promise(
                            store.data(),
                            promise,
                            PromiseSettlement::Reject(runtime_error_value(
                                store.data(),
                                "TypeError: promise reaction handler failed".to_string(),
                            )),
                        ),
                    }
                } else {
                    let settlement = passive_reaction_settlement(reaction_type, argument);
                    settle_promise(store.data(), promise, settlement);
                }
            }
            Some(Microtask::PromiseResolveThenable {
                promise,
                thenable,
                then,
            }) => {
                let (resolve, reject) = create_promise_resolving_functions(store.data(), promise);
                if call_host_function_from_store(
                    store,
                    func_table,
                    memory,
                    shadow_sp_global,
                    then,
                    resolve,
                )
                .is_none()
                {
                    settle_promise(store.data(), promise, PromiseSettlement::Reject(reject));
                }
                let _ = thenable;
            }
            Some(Microtask::MicrotaskCallback { callback }) => {
                if value::is_callable(callback) {
                    let _ = call_host_function_from_store(
                        store,
                        func_table,
                        memory,
                        shadow_sp_global,
                        callback,
                        value::encode_undefined(),
                    );
                }
            }
            Some(Microtask::AsyncResume {
                fn_table_idx,
                continuation,
                state,
                resume_val,
                is_rejected,
            }) => {
                resume_async_function_from_store(
                    store,
                    func_table,
                    fn_table_idx,
                    continuation,
                    state,
                    resume_val,
                    is_rejected,
                );
            }
            Some(Microtask::CleanupFinalizationRegistry { .. }) => {
                // In the store variant, cleanup is handled separately via pending_cleanup_callbacks
            }
            None => break,
        }
    }
    // ── §27.2.1.9 HostPromiseRejectionTracker ────────────────────────────
    // 微任务队列排空后，扫描 promise table 检测未处理的 rejection 并输出警告
    let unhandled: Vec<i64> = {
        let table = store
            .data()
            .promise_table
            .lock()
            .expect("promise table mutex");
        table
            .iter()
            .filter(|e| e.is_promise && !e.handled)
            .filter_map(|e| match &e.state {
                PromiseState::Rejected(reason) => Some(*reason),
                _ => None,
            })
            .collect()
    };
    for reason in unhandled {
        // store 变体无法直接调用 render_value，使用简化格式
        let msg = if value::is_string(reason) {
            String::from("<string>")
        } else if value::is_f64(reason) {
            format!("{}", f64::from_bits(reason as u64))
        } else if value::is_object(reason) {
            String::from("Object")
        } else {
            format!("0x{:016x}", reason as u64)
        };
        eprintln!("UnhandledPromiseRejectionWarning: {msg}");
    }
}

pub(crate) fn call_host_function_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    func_table: &Table,
    handler: i64,
    argument: i64,
) -> Option<i64> {
    if value::is_native_callable(handler) {
        return call_native_callable_from_caller(caller, handler, Some(argument));
    }

    let (func_idx, env_obj) = if value::is_closure(handler) {
        let idx = value::decode_closure_idx(handler);
        let closures = caller.data().closures.lock().unwrap();
        let entry = &closures[idx as usize];
        (entry.func_idx, entry.env_obj)
    } else if value::is_function(handler) {
        (
            value::decode_function_idx(handler),
            value::encode_undefined(),
        )
    } else if value::is_bound(handler) {
        let bound_idx = value::decode_bound_idx(handler);
        let bound = caller.data().bound_objects.lock().unwrap();
        let record = &bound[bound_idx as usize];
        (
            value::decode_function_idx(record.target_func),
            record.bound_this,
        )
    } else {
        return None;
    };

    let shadow_sp_global = caller
        .get_export("__shadow_sp")
        .and_then(|e| e.into_global());
    let saved_sp = shadow_sp_global
        .as_ref()
        .and_then(|g| g.get(&mut *caller).i32())
        .unwrap_or(0);

    if let Some(sp_global) = &shadow_sp_global {
        let sp = saved_sp;
        let new_sp = sp + 8;
        if let Some(Extern::Memory(memory)) = caller.get_export("memory") {
            let data = memory.data_mut(&mut *caller);
            let offset = sp as usize;
            if offset + 8 <= data.len() {
                data[offset..offset + 8].copy_from_slice(&argument.to_le_bytes());
            }
        }
        let _ = sp_global.set(&mut *caller, Val::I32(new_sp));
    }

    let func_ref = func_table.get(&mut *caller, func_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else {
        if let Some(sp_global) = &shadow_sp_global {
            let _ = sp_global.set(&mut *caller, Val::I32(saved_sp));
        }
        return None;
    };
    let mut results = [Val::I64(0)];
    if let Err(err) = func.call(
        &mut *caller,
        &[
            Val::I64(env_obj),
            Val::I64(value::encode_undefined()),
            Val::I32(saved_sp),
            Val::I32(1),
        ],
        &mut results,
    ) {
        set_runtime_error(
            caller.data(),
            format!("promise reaction handler error: {err}"),
        );
        if let Some(sp_global) = &shadow_sp_global {
            let _ = sp_global.set(&mut *caller, Val::I32(saved_sp));
        }
        return None;
    }

    if let Some(sp_global) = &shadow_sp_global {
        let _ = sp_global.set(&mut *caller, Val::I32(saved_sp));
    }

    results[0].i64()
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

pub(crate) fn resume_async_function_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    func_table: &Table,
    fn_table_idx: u32,
    continuation: i64,
    state: u32,
    resume_val: i64,
    is_rejected: bool,
) {
    {
        let cont_handle = value::decode_object_handle(continuation) as usize;
        let mut c_table = caller
            .data()
            .continuation_table
            .lock()
            .expect("continuation table mutex");
        if let Some(entry) = c_table.get_mut(cont_handle) {
            while entry.captured_vars.len() < 2 {
                entry.captured_vars.push(value::encode_undefined());
            }
            entry.captured_vars[0] = value::encode_f64(state as f64);
            entry.captured_vars[1] = value::encode_bool(is_rejected);
        }
    }
    let func_ref = func_table.get(&mut *caller, fn_table_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else { return };
    let mut results = [Val::I64(0)];
    let _ = func.call(
        &mut *caller,
        &[
            Val::I64(continuation),
            Val::I64(resume_val),
            Val::I32(0),
            Val::I32(0),
        ],
        &mut results,
    );
}

pub(crate) fn call_host_function_from_store(
    store: &mut Store<RuntimeState>,
    func_table: &Table,
    memory: &Memory,
    shadow_sp_global: &Global,
    handler: i64,
    argument: i64,
) -> Option<i64> {
    let (func_idx, env_obj) = if value::is_closure(handler) {
        let idx = value::decode_closure_idx(handler);
        let closures = store.data().closures.lock().unwrap();
        let entry = &closures[idx as usize];
        (entry.func_idx, entry.env_obj)
    } else if value::is_function(handler) {
        (
            value::decode_function_idx(handler),
            value::encode_undefined(),
        )
    } else if value::is_bound(handler) {
        let bound_idx = value::decode_bound_idx(handler);
        let bound = store.data().bound_objects.lock().unwrap();
        let record = &bound[bound_idx as usize];
        (
            value::decode_function_idx(record.target_func),
            record.bound_this,
        )
    } else {
        return None;
    };

    let saved_sp = shadow_sp_global.get(&mut *store).i32().unwrap_or(0);
    {
        let data = memory.data_mut(&mut *store);
        let offset = saved_sp as usize;
        if offset + 8 <= data.len() {
            data[offset..offset + 8].copy_from_slice(&argument.to_le_bytes());
        }
    }
    let new_sp = saved_sp + 8;
    let _ = shadow_sp_global.set(&mut *store, Val::I32(new_sp));

    let func_ref = func_table.get(&mut *store, func_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else {
        let _ = shadow_sp_global.set(&mut *store, Val::I32(saved_sp));
        return None;
    };
    let mut results = [Val::I64(0)];
    if let Err(err) = func.call(
        &mut *store,
        &[
            Val::I64(env_obj),
            Val::I64(value::encode_undefined()),
            Val::I32(saved_sp),
            Val::I32(1),
        ],
        &mut results,
    ) {
        set_runtime_error(
            store.data(),
            format!("promise reaction handler error: {err}"),
        );
        let _ = shadow_sp_global.set(&mut *store, Val::I32(saved_sp));
        return None;
    }

    let _ = shadow_sp_global.set(&mut *store, Val::I32(saved_sp));

    results[0].i64()
}

pub(crate) fn resume_async_function_from_store(
    store: &mut Store<RuntimeState>,
    func_table: &Table,
    fn_table_idx: u32,
    continuation: i64,
    state: u32,
    resume_val: i64,
    is_rejected: bool,
) {
    {
        let cont_handle = value::decode_object_handle(continuation) as usize;
        let mut c_table = store
            .data()
            .continuation_table
            .lock()
            .expect("continuation table mutex");
        if let Some(entry) = c_table.get_mut(cont_handle) {
            while entry.captured_vars.len() < 2 {
                entry.captured_vars.push(value::encode_undefined());
            }
            entry.captured_vars[0] = value::encode_f64(state as f64);
            entry.captured_vars[1] = value::encode_bool(is_rejected);
        }
    }
    let func_ref = func_table.get(&mut *store, fn_table_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else { return };
    let mut results = [Val::I64(0)];
    let _ = func.call(
        &mut *store,
        &[
            Val::I64(continuation),
            Val::I64(resume_val),
            Val::I32(0),
            Val::I32(0),
        ],
        &mut results,
    );
}
