use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::*;

pub(crate) fn define_promise_combinators(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let promise_all_fn = Func::wrap(
        &mut store,
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
    linker.define(&mut store, "env", "promise_all", promise_all_fn)?;

    // ── Import 123: promise_race(i64, i64) -> i64 ────────────────────────────────
    let promise_race_fn = Func::wrap(
        &mut store,
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
    linker.define(&mut store, "env", "promise_race", promise_race_fn)?;

    // ── Import 124: promise_all_settled(i64, i64) -> i64 ─────────────────────────
    let promise_all_settled_fn = Func::wrap(
        &mut store,
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
    linker.define(
        &mut store,
        "env",
        "promise_all_settled",
        promise_all_settled_fn,
    )?;

    // ── Import 125: promise_any(i64, i64) -> i64 ─────────────────────────────────
    let promise_any_fn = Func::wrap(
        &mut store,
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
    linker.define(&mut store, "env", "promise_any", promise_any_fn)?;

    Ok(())
}
