use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use swc_core::ecma::ast as swc_ast;
use wasmtime::{Caller, Extern, Func, FuncType, Global, Memory, Store, Table, Val};

use crate::types::*;
use crate::runtime::string_utils::*;
use crate::runtime::memory::*;
use crate::runtime::object_ops::{read_object_property_by_name, find_property_slot_by_name_id, collect_own_property_names, collect_own_property_values};
use crate::runtime::conversions::{to_number, get_string_value, type_tag};
use crate::runtime::format::format_number_js;
use crate::runtime::array_ops::write_array_elem;
use crate::runtime::function_ops::{read_shadow_arg, call_wasm_callback, resolve_and_call, resolve_callable_and_call, raw_promise_handle, insert_promise_entry};
use crate::runtime::microtask::{set_runtime_error, call_host_function_from_caller, drain_microtasks_from_caller};
use crate::runtime::promise_core::{advance_object_iterator_from_caller, create_async_generator_identity, create_map_set_method, create_date_method, create_weakmap_method, create_weakset_method, read_date_ms, write_date_ms, date_args_to_ms, set_host_data_property_from_caller, is_object_key, call_date_method_from_caller, call_map_set_method_from_caller, call_native_callable_from_caller, call_native_callable_with_args_from_caller, call_weakmap_method_from_caller, call_weakset_method_from_caller, perform_eval_from_caller, try_compiled_eval_from_caller, reserve_eval_data_segment, cached_eval_wasm, compiled_eval_import, format_eval_error, call_eval_function_from_caller, create_eval_function, eval_module_items, eval_stmt, eval_block, eval_expr, eval_lit, eval_unary, eval_binary, eval_logical, eval_call, eval_call_function, eval_function_stmt, eval_function_from_decl, eval_function_block, eval_assign, eval_read_binding, eval_write_binding, eval_declare_local, eval_scope_has_strict_marker, pat_ident_name, runtime_module_has_use_strict_directive, ms_to_datetime_utc, ms_to_datetime_local, read_weakmap_handle, read_weakset_handle};
use wjsm_ir::{constants, value};

pub(crate) fn eval_to_number(val: i64) -> f64 {
    if value::is_f64(val) {
        f64::from_bits(val as u64)
    } else if value::is_bool(val) {
        if value::decode_bool(val) { 1.0 } else { 0.0 }
    } else if value::is_null(val) {
        0.0
    } else {
        f64::NAN
    }
}

pub(crate) fn eval_to_string(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    if value::is_string(val) {
        read_value_string_bytes(caller, val)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default()
    } else if value::is_f64(val) {
        let number = f64::from_bits(val as u64);
        if number.fract() == 0.0 {
            format!("{}", number as i64)
        } else {
            number.to_string()
        }
    } else if value::is_bool(val) {
        value::decode_bool(val).to_string()
    } else if value::is_null(val) {
        "null".to_string()
    } else if value::is_undefined(val) {
        "undefined".to_string()
    } else {
        "[object Object]".to_string()
    }
}

pub(crate) fn promise_entry_mut(table: &mut [PromiseEntry], handle: usize) -> Option<&mut PromiseEntry> {
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

pub(crate) fn alloc_promise_from_caller(caller: &mut Caller<'_, RuntimeState>, entry: PromiseEntry) -> i64 {
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

enum AsyncGeneratorPumpAction {
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

pub(crate) fn pump_async_generator_from_caller(caller: &mut Caller<'_, RuntimeState>, generator: i64) {
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

pub(crate) fn decrement_combinator_remaining(state: &RuntimeState, context: usize) -> Option<(i64, i64)> {
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

    if value::is_object(resolution)
        || value::is_function(resolution)
        || value::is_callable(resolution)
    {
        if let Some(ptr) = resolve_handle(caller, resolution) {
            if let Some(then) = read_object_property_by_name(caller, ptr, "then") {
                if value::is_callable(then) {
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
            }
        }
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

    if value::is_object(resolution)
        || value::is_function(resolution)
        || value::is_callable(resolution)
    {
        if let Some(ptr) =
            resolve_handle_from_store(store, memory, obj_table_ptr_global, resolution)
        {
            if let Some(then) = read_object_property_by_name_from_store(store, memory, ptr, "then")
            {
                if value::is_callable(then) {
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
            }
        }
    }

    settle_promise(
        store.data(),
        promise,
        PromiseSettlement::Fulfill(resolution),
    );
}

pub(crate) fn passive_reaction_settlement(reaction_type: ReactionType, argument: i64) -> PromiseSettlement {
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

