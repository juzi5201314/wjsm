use wasmtime::*;
use wjsm_ir::value;
use std::sync::Arc;
use std::sync::Mutex;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn alloc_promise(caller: &mut Caller<'_, RuntimeState>, entry: PromiseEntry) -> i64 {
    let promise = alloc_object(caller, 0);
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

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let promise_create_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let promise = alloc_object(&mut caller, 0);
            let handle = value::decode_object_handle(promise) as usize;
            let mut table = caller
                .data()
                .promise_table
                .lock()
                .expect("promise table mutex");
            insert_promise_entry(&mut table, handle, PromiseEntry::pending());
            promise
        },
    );

    // ── Import 117: promise_instance_resolve(i64, i64) -> () ───────────────

    let promise_instance_resolve_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, promise: i64, value: i64| {
            resolve_promise_from_caller(&mut caller, promise, value);
        },
    );

    // ── Import 118: promise_instance_reject(i64, i64) -> () ────────────────

    let promise_instance_reject_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, promise: i64, reason: i64| {
            settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
        },
    );

    // ── Import 142: promise_create_resolve_function(i64) -> i64 ─────────────

    let promise_create_resolve_function_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, promise: i64| -> i64 {
            let handle = raw_promise_handle(promise);
            let already_resolved = {
                let mut table = caller
                    .data()
                    .promise_table
                    .lock()
                    .expect("promise table mutex");
                let Some(entry) = promise_entry_mut(&mut table, handle) else {
                    return value::encode_undefined();
                };
                let record = Arc::new(Mutex::new(false));
                entry.constructor_resolver = Some(Arc::clone(&record));
                record
            };
            create_promise_resolving_function(
                caller.data(),
                promise,
                already_resolved,
                PromiseResolvingKind::Fulfill,
            )
        },
    );

    // ── Import 143: promise_create_reject_function(i64) -> i64 ──────────────

    let promise_create_reject_function_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, promise: i64| -> i64 {
            let handle = raw_promise_handle(promise);
            let already_resolved = {
                let mut table = caller
                    .data()
                    .promise_table
                    .lock()
                    .expect("promise table mutex");
                let Some(entry) = promise_entry_mut(&mut table, handle) else {
                    return value::encode_undefined();
                };
                entry
                    .constructor_resolver
                    .clone()
                    .unwrap_or_else(|| Arc::new(Mutex::new(false)))
            };
            create_promise_resolving_function(
                caller.data(),
                promise,
                already_resolved,
                PromiseResolvingKind::Reject,
            )
        },
    );

    // ── Import 119: promise_then(i64, i64, i64) -> i64 ─────────────────────

    let promise_then_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>,
         promise: i64,
         on_fulfilled: i64,
         on_rejected: i64|
         -> i64 {
            let handle = raw_promise_handle(promise);
            let result_promise = alloc_object(&mut caller, 0);
            let result_handle = value::decode_object_handle(result_promise) as usize;
            let mut queued = None;
            {
                let mut table = caller
                    .data()
                    .promise_table
                    .lock()
                    .expect("promise table mutex");
                // §27.2.5.1 — 读取原 promise 的构造器作为 species
                let species_constructor =
                    promise_entry(&table, handle).and_then(|entry| entry.constructor_handle);
                let mut result_entry = PromiseEntry::pending();
                result_entry.constructor_handle = species_constructor;
                insert_promise_entry(&mut table, result_handle, result_entry);
                if let Some(entry) = promise_entry_mut(&mut table, handle) {
                    // §27.2.5.3.1 step 10 — .then() 总是标记为已处理
                    entry.handled = true;
                    match entry.state.clone() {
                        PromiseState::Pending => {
                            entry.fulfill_reactions.push(PromiseReaction::new(
                                on_fulfilled,
                                result_handle as i64,
                                ReactionType::Fulfill,
                            ));
                            entry.reject_reactions.push(PromiseReaction::new(
                                on_rejected,
                                result_handle as i64,
                                ReactionType::Reject,
                            ));
                        }
                        PromiseState::Fulfilled(val) => {
                            queued = Some((ReactionType::Fulfill, on_fulfilled, val));
                        }
                        PromiseState::Rejected(reason) => {
                            queued = Some((ReactionType::Reject, on_rejected, reason));
                        }
                    }
                }
            }
            if let Some((reaction_type, handler, argument)) = queued {
                let mut queue = caller
                    .data()
                    .microtask_queue
                    .lock()
                    .expect("microtask queue mutex");
                queue.push_back(Microtask::PromiseReaction {
                    promise: result_handle as i64,
                    reaction_type,
                    handler,
                    argument,
                });
            }
            result_promise
        },
    );

    // ── Import 120: promise_catch(i64, i64) -> i64 ──────────────────────────

    let promise_catch_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, promise: i64, on_rejected: i64| -> i64 {
            let handle = raw_promise_handle(promise);
            let result_promise = alloc_object(&mut caller, 0);
            let result_handle = value::decode_object_handle(result_promise) as usize;
            let mut queued = None;
            {
                let mut table = caller
                    .data()
                    .promise_table
                    .lock()
                    .expect("promise table mutex");
                // §27.2.5.1 — species-aware: 读取原 promise 的构造器
                let species_constructor =
                    promise_entry(&table, handle).and_then(|entry| entry.constructor_handle);
                let mut result_entry = PromiseEntry::pending();
                result_entry.constructor_handle = species_constructor;
                insert_promise_entry(&mut table, result_handle, result_entry);
                if let Some(entry) = promise_entry_mut(&mut table, handle) {
                    entry.handled = true;
                    match entry.state.clone() {
                        PromiseState::Pending => {
                            entry.fulfill_reactions.push(PromiseReaction::new(
                                value::encode_undefined(),
                                result_handle as i64,
                                ReactionType::Fulfill,
                            ));
                            entry.reject_reactions.push(PromiseReaction::new(
                                on_rejected,
                                result_handle as i64,
                                ReactionType::Reject,
                            ));
                        }
                        PromiseState::Fulfilled(val) => {
                            queued = Some((ReactionType::Fulfill, value::encode_undefined(), val));
                        }
                        PromiseState::Rejected(reason) => {
                            queued = Some((ReactionType::Reject, on_rejected, reason));
                        }
                    }
                }
            }
            if let Some((reaction_type, handler, argument)) = queued {
                let mut queue = caller
                    .data()
                    .microtask_queue
                    .lock()
                    .expect("microtask queue mutex");
                queue.push_back(Microtask::PromiseReaction {
                    promise: result_handle as i64,
                    reaction_type,
                    handler,
                    argument,
                });
            }
            result_promise
        },
    );

    // ── Import 121: promise_finally(i64, i64) -> i64 ────────────────────────

    let promise_finally_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, promise: i64, on_finally: i64| -> i64 {
            let handle = raw_promise_handle(promise);
            let result_promise = alloc_object(&mut caller, 0);
            let result_handle = value::decode_object_handle(result_promise) as usize;
            let mut queued = None;
            {
                let mut table = caller
                    .data()
                    .promise_table
                    .lock()
                    .expect("promise table mutex");
                // §27.2.5.1 — species-aware: 读取原 promise 的构造器
                let species_constructor =
                    promise_entry(&table, handle).and_then(|entry| entry.constructor_handle);
                let mut result_entry = PromiseEntry::pending();
                result_entry.constructor_handle = species_constructor;
                insert_promise_entry(&mut table, result_handle, result_entry);
                if let Some(entry) = promise_entry_mut(&mut table, handle) {
                    entry.handled = true;
                    match entry.state.clone() {
                        PromiseState::Pending => {
                            entry.fulfill_reactions.push(PromiseReaction::new(
                                on_finally,
                                result_handle as i64,
                                ReactionType::FinallyFulfill,
                            ));
                            entry.reject_reactions.push(PromiseReaction::new(
                                on_finally,
                                result_handle as i64,
                                ReactionType::FinallyReject,
                            ));
                        }
                        PromiseState::Fulfilled(val) => {
                            queued = Some((ReactionType::FinallyFulfill, on_finally, val));
                        }
                        PromiseState::Rejected(reason) => {
                            queued = Some((ReactionType::FinallyReject, on_finally, reason));
                        }
                    }
                }
            }
            if let Some((reaction_type, handler, argument)) = queued {
                let mut queue = caller
                    .data()
                    .microtask_queue
                    .lock()
                    .expect("microtask queue mutex");
                queue.push_back(Microtask::PromiseReaction {
                    promise: result_handle as i64,
                    reaction_type,
                    handler,
                    argument,
                });
            }
            result_promise
        },
    );

    // ── Import 122: promise_all(i64, i64) -> i64 ─────────────────────────────────

    let promise_all_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64, arr: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                let mut entry = PromiseEntry::rejected(value::encode_undefined());
                if !value::is_undefined(constructor) && !value::is_null(constructor) {
                    entry.constructor_handle = Some(constructor);
                }
                return alloc_promise(&mut caller, entry);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let mut entry = PromiseEntry::pending();
            if !value::is_undefined(constructor) && !value::is_null(constructor) {
                entry.constructor_handle = Some(constructor);
            }
            let result_promise = alloc_promise(&mut caller, entry);

            if len == 0 {
                let empty_arr = alloc_array(&mut caller, 0);
                if let Some(empty_ptr) = resolve_array_ptr(&mut caller, empty_arr) {
                    write_array_length(&mut caller, empty_ptr, 0);
                }
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Fulfill(empty_arr),
                );
                return result_promise;
            }

            let result_array = alloc_array(&mut caller, len);
            if let Some(result_ptr) = resolve_array_ptr(&mut caller, result_array) {
                write_array_length(&mut caller, result_ptr, len);
            }
            let context = create_combinator_context(caller.data(), result_promise, result_array);
            let result_handle = raw_promise_handle(result_promise) as i64;
            let elems: Vec<i64> = (0..len)
                .map(|i| {
                    read_array_elem(&mut caller, ptr, i).unwrap_or_else(value::encode_undefined)
                })
                .collect();
            let mut remaining = 0usize;
            let mut rejected = None;

            for (index, elem) in elems.iter().copied().enumerate() {
                let mut fulfilled = None;
                let mut rejected_elem = None;
                let mut pending = false;
                let mut known_promise = false;

                if value::is_object(elem) {
                    let elem_handle = value::decode_object_handle(elem) as usize;
                    let mut table = caller
                        .data()
                        .promise_table
                        .lock()
                        .expect("promise table mutex");
                    if let Some(entry) = promise_entry_mut(&mut table, elem_handle) {
                        known_promise = true;
                        entry.handled = true; // §27.2.4.1.1 — 标记所有已知 promise 为已处理
                        match entry.state.clone() {
                            PromiseState::Fulfilled(value) => fulfilled = Some(value),
                            PromiseState::Rejected(reason) => rejected_elem = Some(reason),
                            PromiseState::Pending => {
                                pending = true;
                                let handler = create_combinator_reaction_handler(
                                    caller.data(),
                                    context,
                                    index,
                                    PromiseCombinatorReactionKind::AllFulfill,
                                );
                                entry.fulfill_reactions.push(PromiseReaction::new(
                                    handler,
                                    result_handle,
                                    ReactionType::Fulfill,
                                ));
                                entry.reject_reactions.push(PromiseReaction::new(
                                    value::encode_undefined(),
                                    result_handle,
                                    ReactionType::Reject,
                                ));
                            }
                        }
                    }
                }

                if pending {
                    remaining += 1;
                } else if let Some(reason) = rejected_elem {
                    rejected.get_or_insert(reason);
                } else {
                    let value = fulfilled.unwrap_or(elem);
                    if let Some(result_ptr) = resolve_array_ptr(&mut caller, result_array) {
                        write_array_elem(&mut caller, result_ptr, index as u32, value);
                    }
                    let _ = known_promise;
                }
            }

            set_combinator_remaining(caller.data(), context, remaining);
            if let Some(reason) = rejected {
                mark_combinator_settled(caller.data(), context);
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Reject(reason),
                );
            } else if remaining == 0 {
                mark_combinator_settled(caller.data(), context);
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Fulfill(result_array),
                );
            }

            result_promise
        },
    );

    // ── Import 123: promise_race(i64, i64) -> i64 ────────────────────────────────

    let promise_race_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64, arr: i64| -> i64 {
            let mut entry = PromiseEntry::pending();
            if !value::is_undefined(constructor) && !value::is_null(constructor) {
                entry.constructor_handle = Some(constructor);
            }
            let result_promise = alloc_promise(&mut caller, entry);
            let result_handle = raw_promise_handle(result_promise) as i64;
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Reject(value::encode_undefined()),
                );
                return result_promise;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);

            for index in 0..len {
                let elem = read_array_elem(&mut caller, ptr, index)
                    .unwrap_or_else(value::encode_undefined);
                if value::is_object(elem) {
                    let elem_handle = value::decode_object_handle(elem) as usize;
                    let mut immediate = None;
                    {
                        let mut table = caller
                            .data()
                            .promise_table
                            .lock()
                            .expect("promise table mutex");
                        if let Some(entry) = promise_entry_mut(&mut table, elem_handle) {
                            entry.handled = true; // 标记所有已知 promise 为已处理
                            match entry.state.clone() {
                                PromiseState::Fulfilled(value) => {
                                    immediate = Some(PromiseSettlement::Fulfill(value));
                                }
                                PromiseState::Rejected(reason) => {
                                    immediate = Some(PromiseSettlement::Reject(reason));
                                }
                                PromiseState::Pending => {
                                    entry.fulfill_reactions.push(PromiseReaction::new(
                                        value::encode_undefined(),
                                        result_handle,
                                        ReactionType::Fulfill,
                                    ));
                                    entry.reject_reactions.push(PromiseReaction::new(
                                        value::encode_undefined(),
                                        result_handle,
                                        ReactionType::Reject,
                                    ));
                                }
                            }
                        } else {
                            immediate = Some(PromiseSettlement::Fulfill(elem));
                        }
                    }
                    if let Some(settlement) = immediate {
                        settle_promise(caller.data(), result_promise, settlement);
                        return result_promise;
                    }
                } else {
                    resolve_promise_from_caller(&mut caller, result_promise, elem);
                    return result_promise;
                }
            }
            result_promise
        },
    );

    // ── Import 124: promise_all_settled(i64, i64) -> i64 ─────────────────────────

    let promise_all_settled_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64, arr: i64| -> i64 {
            let mut entry = PromiseEntry::pending();
            if !value::is_undefined(constructor) && !value::is_null(constructor) {
                entry.constructor_handle = Some(constructor);
            }
            let result_promise = alloc_promise(&mut caller, entry);
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Reject(value::encode_undefined()),
                );
                return result_promise;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let result_array = alloc_array(&mut caller, len);
            if let Some(result_ptr) = resolve_array_ptr(&mut caller, result_array) {
                write_array_length(&mut caller, result_ptr, len);
            }
            let context = create_combinator_context(caller.data(), result_promise, result_array);
            let result_handle = raw_promise_handle(result_promise) as i64;
            let elems: Vec<i64> = (0..len)
                .map(|i| {
                    read_array_elem(&mut caller, ptr, i).unwrap_or_else(value::encode_undefined)
                })
                .collect();
            let mut remaining = 0usize;

            for (index, elem) in elems.iter().copied().enumerate() {
                let mut outcome = Some(("fulfilled", "value", elem));
                let mut pending = false;

                if value::is_object(elem) {
                    let elem_handle = value::decode_object_handle(elem) as usize;
                    let mut table = caller
                        .data()
                        .promise_table
                        .lock()
                        .expect("promise table mutex");
                    if let Some(entry) = promise_entry_mut(&mut table, elem_handle) {
                        entry.handled = true; // 标记所有已知 promise 为已处理
                        match entry.state.clone() {
                            PromiseState::Fulfilled(value) => {
                                outcome = Some(("fulfilled", "value", value))
                            }
                            PromiseState::Rejected(reason) => {
                                outcome = Some(("rejected", "reason", reason))
                            }
                            PromiseState::Pending => {
                                pending = true;
                                outcome = None;
                                let fulfill_handler = create_combinator_reaction_handler(
                                    caller.data(),
                                    context,
                                    index,
                                    PromiseCombinatorReactionKind::AllSettledFulfill,
                                );
                                let reject_handler = create_combinator_reaction_handler(
                                    caller.data(),
                                    context,
                                    index,
                                    PromiseCombinatorReactionKind::AllSettledReject,
                                );
                                entry.fulfill_reactions.push(PromiseReaction::new(
                                    fulfill_handler,
                                    result_handle,
                                    ReactionType::Fulfill,
                                ));
                                entry.reject_reactions.push(PromiseReaction::new(
                                    reject_handler,
                                    result_handle,
                                    ReactionType::Reject,
                                ));
                            }
                        }
                    }
                }

                if pending {
                    remaining += 1;
                    continue;
                }

                if let Some((status, value_name, value)) = outcome {
                    let record =
                        alloc_promise_all_settled_result(&mut caller, status, value_name, value);
                    if let Some(result_ptr) = resolve_array_ptr(&mut caller, result_array) {
                        write_array_elem(&mut caller, result_ptr, index as u32, record);
                    }
                }
            }

            set_combinator_remaining(caller.data(), context, remaining);
            if remaining == 0 {
                mark_combinator_settled(caller.data(), context);
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Fulfill(result_array),
                );
            }
            result_promise
        },
    );

    // ── Import 125: promise_any(i64, i64) -> i64 ─────────────────────────────────

    let promise_any_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64, arr: i64| -> i64 {
            let mut entry = PromiseEntry::pending();
            if !value::is_undefined(constructor) && !value::is_null(constructor) {
                entry.constructor_handle = Some(constructor);
            }
            let result_promise = alloc_promise(&mut caller, entry);
            let result_handle = raw_promise_handle(result_promise) as i64;
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Reject(value::encode_undefined()),
                );
                return result_promise;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let errors_array = alloc_array(&mut caller, len);
            if let Some(errors_ptr) = resolve_array_ptr(&mut caller, errors_array) {
                write_array_length(&mut caller, errors_ptr, len);
            }
            if len == 0 {
                let aggregate = alloc_aggregate_error(&mut caller, errors_array);
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Reject(aggregate),
                );
                return result_promise;
            }

            let context = create_combinator_context(caller.data(), result_promise, errors_array);
            let elems: Vec<i64> = (0..len)
                .map(|i| {
                    read_array_elem(&mut caller, ptr, i).unwrap_or_else(value::encode_undefined)
                })
                .collect();
            let mut remaining = len as usize;
            let mut fulfilled = None;

            for (index, elem) in elems.iter().copied().enumerate() {
                let mut rejected_reason = None;
                let mut pending = false;
                let mut known_promise = false;

                if value::is_object(elem) {
                    let elem_handle = value::decode_object_handle(elem) as usize;
                    let mut table = caller
                        .data()
                        .promise_table
                        .lock()
                        .expect("promise table mutex");
                    if let Some(entry) = promise_entry_mut(&mut table, elem_handle) {
                        known_promise = true;
                        entry.handled = true; // 标记所有已知 promise 为已处理
                        match entry.state.clone() {
                            PromiseState::Fulfilled(value) => fulfilled = Some(value),
                            PromiseState::Rejected(reason) => rejected_reason = Some(reason),
                            PromiseState::Pending => {
                                pending = true;
                                let reject_handler = create_combinator_reaction_handler(
                                    caller.data(),
                                    context,
                                    index,
                                    PromiseCombinatorReactionKind::AnyReject,
                                );
                                entry.fulfill_reactions.push(PromiseReaction::new(
                                    value::encode_undefined(),
                                    result_handle,
                                    ReactionType::Fulfill,
                                ));
                                entry.reject_reactions.push(PromiseReaction::new(
                                    reject_handler,
                                    result_handle,
                                    ReactionType::Reject,
                                ));
                            }
                        }
                    }
                }

                if fulfilled.is_some() {
                    break;
                }
                if pending {
                    continue;
                }
                if let Some(reason) = rejected_reason {
                    if let Some(errors_ptr) = resolve_array_ptr(&mut caller, errors_array) {
                        write_array_elem(&mut caller, errors_ptr, index as u32, reason);
                    }
                    remaining = remaining.saturating_sub(1);
                } else if !known_promise {
                    fulfilled = Some(elem);
                    break;
                }
            }

            set_combinator_remaining(caller.data(), context, remaining);
            if let Some(value) = fulfilled {
                mark_combinator_settled(caller.data(), context);
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Fulfill(value),
                );
            } else if remaining == 0 {
                mark_combinator_settled(caller.data(), context);
                let aggregate = alloc_aggregate_error(&mut caller, errors_array);
                settle_promise(
                    caller.data(),
                    result_promise,
                    PromiseSettlement::Reject(aggregate),
                );
            }
            result_promise
        },
    );

    // ── Import 126: promise_resolve_static(i64, i64) -> i64 ────────────
    // §27.2.4.6 Promise.resolve(C, x) — species-aware

    let promise_resolve_static_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64, val: i64| -> i64 {
            // 若 x 是 promise，检查 SameValue(x.constructor, C)
            if is_promise_value(caller.data(), val) {
                let handle = raw_promise_handle(val);
                let table = caller
                    .data()
                    .promise_table
                    .lock()
                    .expect("promise table mutex");
                if let Some(entry) = promise_entry(&table, handle) {
                    let matches =
                        match (&entry.constructor_handle, value::is_undefined(constructor)) {
                            (None, true) => true,                        // 都是内建 Promise
                            (Some(_), true) => false,                    // 子类 vs 内建
                            (None, false) => false,                      // 内建 vs 子类
                            (Some(ctor), false) => *ctor == constructor, // 同一子类
                        };
                    drop(table);
                    if matches {
                        return val;
                    }
                } else {
                    drop(table);
                }
            }
            // NewPromiseCapability(C) + resolve(x)
            let mut entry = PromiseEntry::pending();
            if !value::is_undefined(constructor) && !value::is_null(constructor) {
                entry.constructor_handle = Some(constructor);
            }
            let promise = alloc_promise_from_caller(&mut caller, entry);
            resolve_promise_from_caller(&mut caller, promise, val);
            promise
        },
    );

    // ── Import 127: promise_reject_static(i64, i64) -> i64 ────────────
    // §27.2.4.5 Promise.reject(C, r) — species-aware

    let promise_reject_static_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64, reason: i64| -> i64 {
            // NewPromiseCapability(C) + reject(r)
            let mut entry = PromiseEntry::rejected(reason);
            if !value::is_undefined(constructor) && !value::is_null(constructor) {
                entry.constructor_handle = Some(constructor);
            }
            alloc_promise_from_caller(&mut caller, entry)
        },
    );

    // ── Import 145: promise_with_resolvers(i64) -> i64 ────────────────
    // §27.2.3.9 Promise.withResolvers(C) — ES2024

    let promise_with_resolvers_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, constructor: i64| -> i64 {
            let (promise, resolve, reject) =
                new_promise_capability_from_caller(&mut caller, constructor);
            // 创建 { promise, resolve, reject } 对象
            let obj = alloc_host_object_from_caller(&mut caller, 3);
            define_host_data_property_from_caller(&mut caller, obj, "promise", promise);
            define_host_data_property_from_caller(&mut caller, obj, "resolve", resolve);
            define_host_data_property_from_caller(&mut caller, obj, "reject", reject);
            obj
        },
    );

    // ── Import 128: is_promise(i64) -> i64 ──────────────────────────────────

    let is_promise_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            value::encode_bool(is_promise_value(caller.data(), val))
        },
    );

    // ── Import 144: is_callable(i64) -> i64 ──────────────────────────────────
    // ECMAScript §7.2.3 IsCallable(argument) → boolean

    vec![
        (116, promise_create_fn),
        (117, promise_instance_resolve_fn),
        (118, promise_instance_reject_fn),
        (119, promise_then_fn),
        (120, promise_catch_fn),
        (121, promise_finally_fn),
        (122, promise_all_fn),
        (123, promise_race_fn),
        (124, promise_all_settled_fn),
        (125, promise_any_fn),
        (126, promise_resolve_static_fn),
        (127, promise_reject_static_fn),
        (128, is_promise_fn),
        (142, promise_create_resolve_function_fn),
        (143, promise_create_reject_function_fn),
        (145, promise_with_resolvers_fn),
    ]
}
