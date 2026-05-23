{
    let promise_create_fn = Func::wrap(
        &mut store,
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
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, promise: i64, value: i64| {
            resolve_promise_from_caller(&mut caller, promise, value);
        },
    );

    // ── Import 118: promise_instance_reject(i64, i64) -> () ────────────────
    let promise_instance_reject_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, promise: i64, reason: i64| {
            settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
        },
    );

    // ── Import 142: promise_create_resolve_function(i64) -> i64 ─────────────
    let promise_create_resolve_function_fn = Func::wrap(
        &mut store,
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
        &mut store,
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
        &mut store,
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
        &mut store,
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
        &mut store,
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

    // ── Import 126: promise_resolve_static(i64, i64) -> i64 ────────────
    // §27.2.4.6 Promise.resolve(C, x) — species-aware
    let promise_resolve_static_fn = Func::wrap(
        &mut store,
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
        &mut store,
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
        &mut store,
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
        &mut store,
        |caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            value::encode_bool(is_promise_value(caller.data(), val))
        },
    );

    // ── Import 144: is_callable(i64) -> i64 ──────────────────────────────────
    // ECMAScript §7.2.3 IsCallable(argument) → boolean
    let is_callable_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            value::encode_bool(is_callable_in_runtime(&mut caller, val))
        },
    );

    // ── Import 129: queue_microtask(i64) -> () ──────────────────────────────
    let queue_microtask_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, callback: i64| {
            let mut queue = caller
                .data()
                .microtask_queue
                .lock()
                .expect("microtask queue mutex");
            queue.push_back(Microtask::MicrotaskCallback { callback });
        },
    );

    // ── Import 130: drain_microtasks() -> () ────────────────────────────────
    let drain_microtasks_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>| {
        let table = caller.get_export("__table").and_then(|e| e.into_table());
        let Some(func_table) = table else { return };
        drain_microtasks_from_caller(&mut caller, &func_table);
    });

    // ── Import 131: async_function_start(i64) -> i64 ────────────────────────
    let async_function_start_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, fn_table_idx: i64| -> i64 {
            let fn_table_idx = if value::is_function(fn_table_idx) {
                value::decode_function_idx(fn_table_idx)
            } else if value::is_closure(fn_table_idx) {
                let idx = value::decode_closure_idx(fn_table_idx);
                let closures = caller.data().closures.lock().unwrap();
                closures.get(idx as usize).map(|e| e.func_idx).unwrap_or(0)
            } else {
                nanbox_to_u32(fn_table_idx)
            };
            let outer_promise = alloc_promise(&mut caller, PromiseEntry::pending());

            let mut c_table = caller
                .data()
                .continuation_table
                .lock()
                .expect("continuation table mutex");
            let cont_handle = c_table.len() as u32;
            c_table.push(ContinuationEntry {
                fn_table_idx,
                outer_promise,
                captured_vars: vec![value::encode_undefined(); 4],
            });
            if let Some(entry) = c_table.get_mut(cont_handle as usize) {
                entry.captured_vars[0] = value::encode_f64(0.0);
                entry.captured_vars[1] = value::encode_bool(false);
                entry.captured_vars[2] = outer_promise;
            }
            drop(c_table);

            let mut queue = caller
                .data()
                .microtask_queue
                .lock()
                .expect("microtask queue mutex");
            queue.push_back(Microtask::AsyncResume {
                fn_table_idx,
                continuation: cont_handle as i64,
                state: 0,
                resume_val: value::encode_undefined(),
                is_rejected: false,
            });

            outer_promise
        },
    );

    // ── Import 132: async_function_resume(i64, i64, i64, i64, i64) -> () ───
    let async_function_resume_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>,
         fn_table_idx: i64,
         continuation: i64,
         state: i64,
         resume_val: i64,
         is_rejected: i64| {
            let resolved_fn_idx = if value::is_function(fn_table_idx) {
                value::decode_function_idx(fn_table_idx)
            } else if value::is_closure(fn_table_idx) {
                let idx = value::decode_closure_idx(fn_table_idx);
                let closures = caller.data().closures.lock().unwrap();
                closures.get(idx as usize).map(|e| e.func_idx).unwrap_or(0)
            } else {
                nanbox_to_u32(fn_table_idx)
            };
            let state = nanbox_to_u32(state);
            let is_rejected_bool = nanbox_to_bool(is_rejected);
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
                    entry.captured_vars[1] = value::encode_bool(is_rejected_bool);
                }
            }
            let mut queue = caller
                .data()
                .microtask_queue
                .lock()
                .expect("microtask queue mutex");
            queue.push_back(Microtask::AsyncResume {
                fn_table_idx: resolved_fn_idx,
                continuation,
                state,
                resume_val,
                is_rejected: is_rejected_bool,
            });
        },
    );

    // ── Import 133: async_function_suspend(i64, i64, i64) -> () ─────────────
    let async_function_suspend_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, continuation: i64, awaited_promise: i64, state: i64| {
            let cont_handle = value::decode_object_handle(continuation) as usize;
            {
                let mut c_table = caller
                    .data()
                    .continuation_table
                    .lock()
                    .expect("continuation table mutex");
                if let Some(entry) = c_table.get_mut(cont_handle) {
                    while entry.captured_vars.len() < 4 {
                        entry.captured_vars.push(value::encode_undefined());
                    }
                    entry.captured_vars[0] = value::encode_f64(state as f64);
                    entry.captured_vars[1] = value::encode_bool(false);
                }
            }
            let cont_fn_idx = {
                let c_table = caller
                    .data()
                    .continuation_table
                    .lock()
                    .expect("continuation table mutex");
                c_table
                    .get(cont_handle)
                    .map(|e| e.fn_table_idx)
                    .unwrap_or(0)
            };

            let awaited_handle = value::decode_object_handle(awaited_promise) as usize;
            let mut p_table = caller
                .data()
                .promise_table
                .lock()
                .expect("promise table mutex");
            if let Some(entry) = promise_entry_mut(&mut p_table, awaited_handle) {
                // §15.8.1 — await 标记 promise 为已处理
                entry.handled = true;
                match &entry.state {
                    PromiseState::Pending => {
                        entry.fulfill_reactions.push(PromiseReaction::new_async(
                            cont_fn_idx as i64,
                            continuation,
                            ReactionType::Fulfill,
                            state,
                        ));
                        entry.reject_reactions.push(PromiseReaction::new_async(
                            cont_fn_idx as i64,
                            continuation,
                            ReactionType::Reject,
                            state,
                        ));
                    }
                    PromiseState::Fulfilled(val) => {
                        let val = *val;
                        drop(p_table);
                        let mut queue = caller
                            .data()
                            .microtask_queue
                            .lock()
                            .expect("microtask queue mutex");
                        queue.push_back(Microtask::AsyncResume {
                            fn_table_idx: cont_fn_idx,
                            continuation,
                            state: state as u32,
                            resume_val: val,
                            is_rejected: false,
                        });
                    }
                    PromiseState::Rejected(reason) => {
                        let reason = *reason;
                        drop(p_table);
                        let mut queue = caller
                            .data()
                            .microtask_queue
                            .lock()
                            .expect("microtask queue mutex");
                        queue.push_back(Microtask::AsyncResume {
                            fn_table_idx: cont_fn_idx,
                            continuation,
                            state: state as u32,
                            resume_val: reason,
                            is_rejected: true,
                        });
                    }
                }
            }
        },
    );

    // ── Import 134: continuation_create(i64, i64, i64) -> i64 ───────────────
    let continuation_create_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>,
         fn_table_idx: i64,
         outer_promise: i64,
         captured_var_count: i64|
         -> i64 {
            let resolved_fn_idx = if value::is_function(fn_table_idx) {
                value::decode_function_idx(fn_table_idx)
            } else if value::is_closure(fn_table_idx) {
                let idx = value::decode_closure_idx(fn_table_idx);
                let closures = caller.data().closures.lock().unwrap();
                closures.get(idx as usize).map(|e| e.func_idx).unwrap_or(0)
            } else {
                nanbox_to_u32(fn_table_idx)
            };
            let mut table = caller
                .data()
                .continuation_table
                .lock()
                .expect("continuation table mutex");
            let handle = table.len() as u32;
            let total_slots = nanbox_to_usize(captured_var_count);
            table.push(ContinuationEntry {
                fn_table_idx: resolved_fn_idx,
                outer_promise,
                captured_vars: vec![value::encode_undefined(); total_slots],
            });
            if let Some(entry) = table.get_mut(handle as usize) {
                entry.captured_vars[0] = value::encode_f64(0.0);
                entry.captured_vars[1] = value::encode_bool(false);
            }
            value::encode_object_handle(handle)
        },
    );

    // ── Import 135: continuation_save_var(i64, i64, i64) -> () ──────────────
    let continuation_save_var_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, continuation: i64, slot: i64, val: i64| {
            let handle = value::decode_object_handle(continuation) as usize;
            let actual_slot = nanbox_to_usize(slot);
            let mut table = caller
                .data()
                .continuation_table
                .lock()
                .expect("continuation table mutex");
            if let Some(entry) = table.get_mut(handle)
                && actual_slot < entry.captured_vars.len() {
                    entry.captured_vars[actual_slot] = val;
                }
        },
    );

    // ── Import 136: continuation_load_var(i64, i64) -> i64 ──────────────────
    let continuation_load_var_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, continuation: i64, slot: i64| -> i64 {
            let handle = value::decode_object_handle(continuation) as usize;
            let actual_slot = nanbox_to_usize(slot);
            let table = caller
                .data()
                .continuation_table
                .lock()
                .expect("continuation table mutex");
            if let Some(entry) = table.get(handle)
                && actual_slot < entry.captured_vars.len() {
                    return entry.captured_vars[actual_slot];
                }
            value::encode_undefined()
        },
    );

    // ── Import 137: async_generator_start(i64) -> i64 ───────────────────────
    let async_generator_start_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, continuation: i64| -> i64 {
            let generator = alloc_object(&mut caller, 4);
            // 设置 [[Prototype]] = AsyncGenerator.prototype
            let async_gen_proto = caller.data().async_gen_prototype;
            if !value::is_undefined(async_gen_proto) {
                if let Some(ptr) = resolve_handle(&mut caller, generator) {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .expect("memory");
                    let data = memory.data_mut(&mut caller);
                    data[ptr + 4..ptr + 8]
                        .copy_from_slice(&value::decode_object_handle(async_gen_proto).to_le_bytes());
                }
            }
            if !value::is_object(generator) {
                return value::encode_undefined();
            }
            let next = create_async_generator_method(
                caller.data(),
                generator,
                AsyncGeneratorCompletionType::Next,
            );
            let ret = create_async_generator_method(
                caller.data(),
                generator,
                AsyncGeneratorCompletionType::Return,
            );
            let throw = create_async_generator_method(
                caller.data(),
                generator,
                AsyncGeneratorCompletionType::Throw,
            );
            let _ = define_host_data_property_from_caller(&mut caller, generator, "next", next);
            let _ = define_host_data_property_from_caller(&mut caller, generator, "return", ret);
            let _ = define_host_data_property_from_caller(&mut caller, generator, "throw", throw);
            let async_iter = create_async_generator_identity(caller.data(), generator);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                generator,
                "Symbol.asyncIterator",
                async_iter,
            );

            let handle = value::decode_object_handle(generator) as usize;
            let mut table = caller
                .data()
                .async_generator_table
                .lock()
                .expect("async generator table mutex");
            if table.len() <= handle {
                table.resize_with(handle + 1, || AsyncGeneratorEntry {
                    state: AsyncGeneratorState::Completed,
                    continuation: value::encode_undefined(),
                    active_request: None,
                    waiting_resume_promise: None,
                    queue: Vec::new(),
                });
            }
            table[handle] = AsyncGeneratorEntry {
                state: AsyncGeneratorState::SuspendedStart,
                continuation,
                active_request: None,
                waiting_resume_promise: None,
                queue: Vec::new(),
            };
            generator
        },
    );

    // ── Import 138: async_generator_next(i64, i64) -> i64 ───────────────────
    let async_generator_next_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            let resume_promise = alloc_promise(&mut caller, PromiseEntry::pending());
            let request_to_fulfill = {
                let mut table = caller
                    .data()
                    .async_generator_table
                    .lock()
                    .expect("async generator table mutex");
                let Some(entry) = table.get_mut(value::decode_object_handle(generator) as usize)
                else {
                    return resume_promise;
                };
                entry.state = AsyncGeneratorState::SuspendedYield;
                entry.waiting_resume_promise = Some(resume_promise);
                entry.active_request.take()
            };
            if let Some(request) = request_to_fulfill {
                let result = alloc_iterator_result_from_caller(&mut caller, value, false);
                resolve_promise_from_caller(&mut caller, request.promise, result);
            }
            pump_async_generator_from_caller(&mut caller, generator);
            resume_promise
        },
    );

    // ── Import 139: async_generator_return(i64, i64) -> i64 ─────────────────
    let async_generator_return_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            let (active, queued) = {
                let mut table = caller
                    .data()
                    .async_generator_table
                    .lock()
                    .expect("async generator table mutex");
                let Some(entry) = table.get_mut(value::decode_object_handle(generator) as usize)
                else {
                    return value::encode_undefined();
                };
                entry.state = AsyncGeneratorState::Completed;
                let active = entry.active_request.take();
                let queued = std::mem::take(&mut entry.queue);
                (active, queued)
            };
            if let Some(request) = active {
                let result = alloc_iterator_result_from_caller(&mut caller, value, true);
                resolve_promise_from_caller(&mut caller, request.promise, result);
            }
            for request in queued {
                match request.completion_type {
                    AsyncGeneratorCompletionType::Throw => settle_promise(
                        caller.data(),
                        request.promise,
                        PromiseSettlement::Reject(request.value),
                    ),
                    _ => {
                        let result = alloc_iterator_result_from_caller(
                            &mut caller,
                            value::encode_undefined(),
                            true,
                        );
                        resolve_promise_from_caller(&mut caller, request.promise, result);
                    }
                }
            }
            value::encode_undefined()
        },
    );

    // ── Import 140: async_generator_throw(i64, i64) -> i64 ──────────────────
    let async_generator_throw_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            let (active, queued) = {
                let mut table = caller
                    .data()
                    .async_generator_table
                    .lock()
                    .expect("async generator table mutex");
                let Some(entry) = table.get_mut(value::decode_object_handle(generator) as usize)
                else {
                    return value::encode_undefined();
                };
                entry.state = AsyncGeneratorState::Completed;
                let active = entry.active_request.take();
                let queued = std::mem::take(&mut entry.queue);
                (active, queued)
            };
            if let Some(request) = active {
                settle_promise(
                    caller.data(),
                    request.promise,
                    PromiseSettlement::Reject(value),
                );
            }
            for request in queued {
                settle_promise(
                    caller.data(),
                    request.promise,
                    PromiseSettlement::Reject(value),
                );
            }
            value::encode_undefined()
        },
    );

    // ── Import 141: native_call(i64, i64, i32, i32) -> i64 ─────────────────
    let native_call_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         callable: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let new_target_val = caller.data().new_target.get();
            caller.data().new_target.set(value::encode_undefined());

            if value::is_proxy(callable) {
                let handle = value::decode_proxy_handle(callable) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform call on a proxy that has been revoked".to_string());
                        return value::encode_undefined();
                    }

                    if !value::is_undefined(new_target_val) {
                        // 构造调用
                        if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                            let trap = read_object_property_by_name(&mut caller, handler_ptr, "construct")
                                .unwrap_or_else(value::encode_undefined);
                            if !value::is_undefined(trap) && !value::is_null(trap) {
                                let arr = alloc_array(&mut caller, args_count as u32);
                                for i in 0..args_count {
                                    let arg = read_shadow_arg(&mut caller, args_base, i as u32);
                                    set_array_elem(&mut caller, arr, i, arg);
                                }
                                let trap_res = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, arr, new_target_val]);
                                return match trap_res {
                                    Ok(res) => {
                                        if !value::is_js_object(res) {
                                            set_runtime_error(caller.data(), "TypeError: Proxy construct trap returned non-object".to_string());
                                            value::encode_undefined()
                                        } else {
                                            res
                                        }
                                    }
                                    Err(e) => {
                                        set_runtime_error(caller.data(), format!("TypeError: Proxy construct trap failed: {}", e));
                                        value::encode_undefined()
                                    }
                                };
                            }
                        }
                        caller.data().new_target.set(new_target_val);
                        let result = resolve_and_call(&mut caller, entry.target, this_val, args_base, args_count);
                        caller.data().new_target.set(value::encode_undefined());
                        return result;
                    } else {
                        // 普通函数调用
                        if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                            let trap = read_object_property_by_name(&mut caller, handler_ptr, "apply")
                                .unwrap_or_else(value::encode_undefined);
                            if !value::is_undefined(trap) && !value::is_null(trap) {
                                let arr = alloc_array(&mut caller, args_count as u32);
                                for i in 0..args_count {
                                    let arg = read_shadow_arg(&mut caller, args_base, i as u32);
                                    set_array_elem(&mut caller, arr, i, arg);
                                }
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, this_val, arr]);
                            return result.unwrap_or_else(|_| {
                                set_runtime_error(caller.data(), "TypeError: Proxy apply trap failed".to_string());
                                value::encode_undefined()
                            });
                            }
                        }
                        return resolve_and_call(&mut caller, entry.target, this_val, args_base, args_count);
                    }
                }
                return value::encode_undefined();
            }

            if !value::is_undefined(new_target_val) {
                caller.data().new_target.set(new_target_val);
            }
            let args = (0..args_count.max(0))
                .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                .collect();
            let result = call_native_callable_with_args_from_caller(&mut caller, callable, this_val, args)
                .unwrap_or_else(value::encode_undefined);
            caller.data().new_target.set(value::encode_undefined());
            result
        },
    );

    // ── Import 146: register_module_namespace(i64, i64) -> () ──────────────
    // 将模块命名空间对象注册到运行时缓存
    let register_module_namespace_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, module_id: i64, namespace_obj: i64| {
            let mid = module_id as u32;
            let mut cache = caller
                .data()
                .module_namespace_cache
                .lock()
                .expect("module namespace cache mutex");
            cache.insert(mid, namespace_obj);
        },
    );

    // ── Import 147: dynamic_import(i64) -> i64 ────────────────────────────
    // 动态导入：查找命名空间对象并返回 resolved Promise
    let dynamic_import_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, module_id: i64| -> i64 {
            let mid = module_id as u32;

            // 创建 Promise 并添加 .then/.catch/.finally 方法
            let promise = alloc_promise(&mut caller, PromiseEntry::pending());
            let then_fn = create_promise_resolving_function(
                caller.data(),
                promise,
                Arc::new(Mutex::new(false)),
                PromiseResolvingKind::Fulfill,
            );
            let catch_fn = create_promise_resolving_function(
                caller.data(),
                promise,
                Arc::new(Mutex::new(false)),
                PromiseResolvingKind::Reject,
            );
            let _ = define_host_data_property_from_caller(&mut caller, promise, "then", then_fn);
            let _ = define_host_data_property_from_caller(&mut caller, promise, "catch", catch_fn);

            // 从缓存查找命名空间对象
            let namespace_obj = {
                let cache = caller
                    .data()
                    .module_namespace_cache
                    .lock()
                    .expect("module namespace cache mutex");
                cache.get(&mid).copied()
            };

            match namespace_obj {
                Some(ns_obj) => {
                    // 直接 resolve Promise（AOT 模式下命名空间对象已构建完成）
                    resolve_promise_from_caller(&mut caller, promise, ns_obj);
                }
                None => {
                    // 模块未找到：reject Promise
                    let error_msg = format!("Cannot find module with id {}", mid);
                    let error_val = runtime_error_value(caller.data(), error_msg);
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(error_val));
                }
            }

            promise
        },
    );

    // ── Import 148/149: eval ────────────────────────────────────────────────
    let eval_direct_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, code: i64, scope_env: i64| -> i64 {
            perform_eval_from_caller(&mut caller, code, Some(scope_env))
        },
    );
    let eval_indirect_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, code: i64| -> i64 {
            perform_eval_from_caller(&mut caller, code, None)
        },
    );

    let jsx_create_element_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, tag: i64, props: i64, children: i64| -> i64 {
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let _ = define_host_data_property_from_caller(
                &mut caller, obj, "type", tag,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller, obj, "props", props,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller, obj, "children", children,
            );
            obj
        },
    );

    // ── Proxy / Reflect ────────────────────────────────────────────────────────
    let proxy_create_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, target: i64, handler: i64| -> i64 {
            if !value::is_js_object(target) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Proxy target must be an object".to_string());
                return value::encode_undefined();
            }
            if !value::is_js_object(handler) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Proxy handler must be an object".to_string());
                return value::encode_undefined();
            }
            let handle;
            {
                let mut table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                handle = table.len() as u32;
                table.push(ProxyEntry { target, handler, revoked: false });
            }
            value::encode_proxy_handle(handle)
        },
    );

    let reflect_get_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64, receiver: i64| -> i64 {
            // Proxy target: 触发 get trap
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'get' on a proxy that has been revoked".to_string());
                        return value::encode_undefined();
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "get")
                            .unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            return call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop, receiver])
                                .unwrap_or_else(|_| value::encode_undefined());
                        }
                    }
                    // 无 trap，转发到 target
                    return reflect_get_impl(&mut caller, entry.target, prop);
                }
                return value::encode_undefined();
            }
            reflect_get_impl(&mut caller, target, prop)
        },
    );


    let proxy_revocable_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, handler: i64| -> i64 {
            if !value::is_js_object(target) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Proxy target must be an object".to_string());
                return value::encode_undefined();
            }
            if !value::is_js_object(handler) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Proxy handler must be an object".to_string());
                return value::encode_undefined();
            }
            let handle = {
                let mut table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                let handle = table.len() as u32;
                table.push(ProxyEntry { target, handler, revoked: false });
                handle
            };
            let proxy_val = value::encode_proxy_handle(handle);
            let revoke_fn = {
                let mut native_callables = caller.data().native_callables.lock().unwrap();
                let idx = native_callables.len() as u32;
                native_callables.push(NativeCallable::ProxyRevoker { proxy_handle: handle });
                value::encode_native_callable_idx(idx)
            };
            let obj = alloc_host_object_from_caller(&mut caller, 2);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "proxy", proxy_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "revoke", revoke_fn);
            obj
        },
    );

    let reflect_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64, val: i64, receiver: i64| -> i64 {
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked { set_runtime_error(caller.data(), "TypeError: Cannot perform 'set' on a proxy that has been revoked".to_string()); return value::encode_bool(false); }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "set").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop, val, receiver]).unwrap_or_else(|_| value::encode_bool(false));
                            return value::encode_bool(nanbox_to_bool(result));
                        }
                    }
                    return reflect_set_impl(&mut caller, entry.target, prop, val);
                }
                return value::encode_bool(false);
            }
            reflect_set_impl(&mut caller, target, prop, val)
        },
    );

    fn reflect_set_impl(caller: &mut Caller<'_, RuntimeState>, target: i64, prop: i64, val: i64) -> i64 {
        let Ok(prop_name) = render_value(caller, prop) else { return value::encode_bool(false); };
        let name_id = find_memory_c_string(caller, &prop_name);
        let existing = name_id.and_then(|id| {
            let obj_ptr = resolve_handle(caller, target)?;
            find_property_slot_by_name_id(caller, obj_ptr, id)
        });
        if let Some((_, flags, _)) = existing {
            let is_accessor = (flags & constants::FLAG_IS_ACCESSOR) != 0;
            if !is_accessor {
                let writable = (flags & constants::FLAG_WRITABLE) != 0;
                if !writable { return value::encode_bool(false); }
            }
        } else if !is_extensible_impl(caller, target) {
            return value::encode_bool(false);
        }
        let _ = define_host_data_property_from_caller(caller, target, &prop_name, val);
        value::encode_bool(true)
    }

    let reflect_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64| -> i64 {
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked { set_runtime_error(caller.data(), "TypeError: Cannot perform 'has' on a proxy that has been revoked".to_string()); return value::encode_bool(false); }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "has").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop]).unwrap_or_else(|_| value::encode_bool(false));
                            return value::encode_bool(nanbox_to_bool(result));
                        }
                    }
                    return reflect_has_impl(&mut caller, entry.target, prop);
                }
                return value::encode_bool(false);
            }
            reflect_has_impl(&mut caller, target, prop)
        },
    );

    fn reflect_has_impl(caller: &mut Caller<'_, RuntimeState>, target: i64, prop: i64) -> i64 {
        let obj_ptr = resolve_handle(caller, target);
        if let Some(ptr) = obj_ptr
            && let Ok(prop_name) = render_value(caller, prop)
                && let Some(name_id) = find_memory_c_string(caller, &prop_name) {
                    let found = find_property_slot_by_name_id(caller, ptr, name_id).is_some();
                    return value::encode_bool(found);
                }
        value::encode_bool(false)
    }

    let reflect_delete_property_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64| -> i64 {
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked { set_runtime_error(caller.data(), "TypeError: Cannot perform 'deleteProperty' on a proxy that has been revoked".to_string()); return value::encode_bool(false); }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "deleteProperty").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop]).unwrap_or_else(|_| value::encode_bool(false));
                            return value::encode_bool(nanbox_to_bool(result));
                        }
                    }
                    return reflect_delete_property_impl(&mut caller, entry.target, prop);
                }
                return value::encode_bool(false);
            }
            reflect_delete_property_impl(&mut caller, target, prop)
        },
    );
    fn reflect_delete_property_impl(caller: &mut Caller<'_, RuntimeState>, target: i64, prop: i64) -> i64 {
        let prop_name = match render_value(caller, prop) {
            Ok(name) => name,
            Err(_) => return value::encode_bool(true),
        };
        let Some(ptr) = resolve_handle(caller, target) else {
            return value::encode_bool(true);
        };
        let Some(name_id) = find_memory_c_string(caller, &prop_name) else {
            return value::encode_bool(true);
        };
        let Some((
            slot_offset, flags, _val,
        )) = find_property_slot_by_name_id(caller, ptr, name_id)
        else {
            return value::encode_bool(true);
        };
        // Not configurable → can't delete
        if (flags & constants::FLAG_CONFIGURABLE) == 0 {
            return value::encode_bool(false);
        }
        // Perform swap-remove
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return value::encode_bool(false);
        };
        let data = memory.data_mut(&mut *caller);
        if ptr + 16 > data.len() || slot_offset + 32 > data.len() {
            return value::encode_bool(false);
        }
        let num_props = u32::from_le_bytes([data[ptr + 12], data[ptr + 13], data[ptr + 14], data[ptr + 15]]) as usize;
        if num_props == 0 {
            return value::encode_bool(true);
        }
        let last_slot_offset = ptr + 16 + (num_props - 1) * 32;
        // Decrement num_props
        data[ptr + 12..ptr + 16].copy_from_slice(&(num_props as u32 - 1).to_le_bytes());
        // If not deleting the last slot, copy last slot over deleted slot
        if slot_offset != last_slot_offset {
            for j in 0..32 {
                data[slot_offset + j] = data[last_slot_offset + j];
            }
        }
        value::encode_bool(true)
    }

    fn extract_array_like_elements(
        caller: &mut Caller<'_, RuntimeState>,
        arr_like: i64,
    ) -> Result<Vec<i64>, String> {
        let mut elements = Vec::new();
        if value::is_array(arr_like) {
            let handle = value::decode_array_handle(arr_like) as usize;
            let Some(ptr) = resolve_handle_idx(caller, handle) else { return Ok(elements); };
            let len = {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else { return Ok(elements); };
                let data = memory.data(&*caller);
                if ptr + 12 > data.len() { return Ok(elements); }
                u32::from_le_bytes([data[ptr + 8], data[ptr + 9], data[ptr + 10], data[ptr + 11]]) as usize
            };
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else { return Ok(elements); };
            for i in 0..len {
                let mut buf = [0u8; 8];
                if memory.read(&mut *caller, ptr + 16 + i * 8, &mut buf).is_ok() {
                    elements.push(i64::from_le_bytes(buf));
                }
            }
        } else if value::is_object(arr_like) || value::is_proxy(arr_like) {
            let len_prop = store_runtime_string(caller, "length".to_string());
            let len_val = reflect_get_impl(caller, arr_like, len_prop);
            let len = if value::is_f64(len_val) {
                value::decode_f64(len_val) as usize
            } else {
                0
            };
            for i in 0..len {
                let idx_prop = value::encode_f64(i as f64);
                let val = reflect_get_impl(caller, arr_like, idx_prop);
                elements.push(val);
            }
        }
        Ok(elements)
    }

    let reflect_apply_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, this_arg: i64, args_array: i64| -> i64 {
            if !is_callable_in_runtime(&mut caller, target) {
                set_runtime_error(caller.data(), "TypeError: Reflect.apply target must be callable".to_string());
                return value::encode_undefined();
            }
            let args = match extract_array_like_elements(&mut caller, args_array) {
                Ok(arr) => arr,
                Err(err) => {
                    set_runtime_error(caller.data(), err);
                    return value::encode_undefined();
                }
            };

            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().unwrap(); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'apply' on a proxy that has been revoked".to_string());
                        return value::encode_undefined();
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "apply").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let arr = alloc_array(&mut caller, args.len() as u32);
                            for (i, &arg) in args.iter().enumerate() {
                                set_array_elem(&mut caller, arr, i as i32, arg);
                            }
                            let trap_result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, this_arg, arr]);
                            return match trap_result {
                                Ok(res) => res,
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: Proxy apply trap failed: {}", e));
                                    value::encode_undefined()
                                }
                            };
                        }
                    }
                    return reflect_apply_impl(&mut caller, entry.target, this_arg, &args);
                }
            }

            reflect_apply_impl(&mut caller, target, this_arg, &args)
        },
    );

    fn reflect_apply_impl(
        caller: &mut Caller<'_, RuntimeState>,
        target: i64,
        this_arg: i64,
        args: &[i64],
    ) -> i64 {
        let shadow_sp_global = caller.get_export("__shadow_sp").and_then(|e| e.into_global()).unwrap();
        let saved_sp = shadow_sp_global.get(&mut *caller).i32().unwrap();
        let memory = caller.get_export("memory").and_then(|e| e.into_memory()).unwrap();
        for (i, &arg) in args.iter().enumerate() {
            memory.write(&mut *caller, (saved_sp + i as i32 * 8) as usize, &arg.to_le_bytes()).unwrap();
        }
        shadow_sp_global.set(&mut *caller, Val::I32(saved_sp + args.len() as i32 * 8)).unwrap();
        let result = resolve_and_call(caller, target, this_arg, saved_sp, args.len() as i32);
        shadow_sp_global.set(&mut *caller, Val::I32(saved_sp)).unwrap();
        result
    }

    let reflect_construct_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, args_array: i64, new_target: i64| -> i64 {
            let n_target = if value::is_undefined(new_target) { target } else { new_target };
            if !is_callable_in_runtime(&mut caller, target) || !is_callable_in_runtime(&mut caller, n_target) {
                set_runtime_error(caller.data(), "TypeError: Reflect.construct target and newTarget must be constructors".to_string());
                return value::encode_undefined();
            }

            let args = match extract_array_like_elements(&mut caller, args_array) {
                Ok(arr) => arr,
                Err(err) => {
                    set_runtime_error(caller.data(), err);
                    return value::encode_undefined();
                }
            };

            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().unwrap(); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'construct' on a proxy that has been revoked".to_string());
                        return value::encode_undefined();
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "construct").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let arr = alloc_array(&mut caller, args.len() as u32);
                            for (i, &arg) in args.iter().enumerate() {
                                set_array_elem(&mut caller, arr, i as i32, arg);
                            }
                            let trap_result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, arr, n_target]);
                            return match trap_result {
                                Ok(res) => {
                                    if !value::is_js_object(res) {
                                        set_runtime_error(caller.data(), "TypeError: Proxy construct trap returned non-object".to_string());
                                        value::encode_undefined()
                                    } else {
                                        res
                                    }
                                }
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: Proxy construct trap failed: {}", e));
                                    value::encode_undefined()
                                }
                            };
                        }
                    }
                    return reflect_construct_impl(&mut caller, entry.target, &args, n_target);
                }
            }

            reflect_construct_impl(&mut caller, target, &args, n_target)
        },
    );

    fn reflect_construct_impl(
        caller: &mut Caller<'_, RuntimeState>,
        target: i64,
        args: &[i64],
        new_target: i64,
    ) -> i64 {
        let this_obj = alloc_host_object_from_caller(caller, 4);
        let proto_prop = store_runtime_string(caller, "prototype".to_string());
        let proto_val = reflect_get_impl(caller, new_target, proto_prop);
        if value::is_object(proto_val) || value::is_array(proto_val) || value::is_proxy(proto_val) || value::is_null(proto_val) {
            let _ = reflect_set_prototype_of_fn_impl(caller, this_obj, proto_val);
        }

        let shadow_sp_global = caller.get_export("__shadow_sp").and_then(|e| e.into_global()).expect("__shadow_sp in reflect_construct_impl");
        let saved_sp = shadow_sp_global.get(&mut *caller).i32().expect("shadow_sp i32 in reflect_construct_impl");
        let memory = caller.get_export("memory").and_then(|e| e.into_memory()).expect("memory in reflect_construct_impl");
        for (i, &arg) in args.iter().enumerate() {
            memory.write(&mut *caller, (saved_sp + i as i32 * 8) as usize, &arg.to_le_bytes()).expect("shadow stack write in reflect_construct_impl");
        }
        shadow_sp_global.set(&mut *caller, Val::I32(saved_sp + args.len() as i32 * 8)).expect("shadow_sp set in reflect_construct_impl");
        let result = resolve_and_call(caller, target, this_obj, saved_sp, args.len() as i32);
        shadow_sp_global.set(&mut *caller, Val::I32(saved_sp)).expect("shadow_sp restore in reflect_construct_impl");

        if value::is_js_object(result) {
            result
        } else {
            this_obj
        }
    }

    let reflect_get_prototype_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64| -> i64 {
            if !value::is_object(target) && !value::is_array(target) && !value::is_function(target) && !value::is_proxy(target) {
                set_runtime_error(caller.data(), "TypeError: Reflect.getPrototypeOf called on non-object".to_string());
                return value::encode_undefined();
            }
            reflect_get_prototype_of_impl(&mut caller, target)
        },
    );

    fn reflect_get_prototype_of_impl(caller: &mut Caller<'_, RuntimeState>, target: i64) -> i64 {
        if value::is_proxy(target) {
            let handle = value::decode_proxy_handle(target) as usize;
            let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
            if let Some(entry) = entry {
                if entry.revoked { set_runtime_error(caller.data(), "TypeError: Cannot perform 'getPrototypeOf' on a proxy that has been revoked".to_string()); return value::encode_undefined(); }
                if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                    let trap = read_object_property_by_name(caller, handler_ptr, "getPrototypeOf").unwrap_or_else(value::encode_undefined);
                    if !value::is_undefined(trap) && !value::is_null(trap) {
                        let res = call_wasm_callback(caller, trap, entry.handler, &[entry.target]).unwrap_or_else(|_| value::encode_null());
                        // Invariant check: if target is non-extensible, returned prototype must match target prototype
                        let ext = is_extensible_impl(caller, entry.target);
                        if !ext {
                            let target_proto = reflect_get_prototype_of_impl(caller, entry.target);
                            if res != target_proto {
                                set_runtime_error(caller.data(), "TypeError: Proxy getPrototypeOf invariant violated: target is not extensible and trap returned different prototype".to_string());
                                return value::encode_null();
                            }
                        }
                        return res;
                    }
                }
                return reflect_get_prototype_of_impl(caller, entry.target);
            }
        }
        let Some(ptr) = resolve_handle(caller, target) else { return value::encode_null(); };
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else { return value::encode_null(); };
        let data = memory.data(&*caller);
        if ptr + 4 > data.len() { return value::encode_null(); }
        let proto_handle = u32::from_le_bytes([data[ptr], data[ptr + 1], data[ptr + 2], data[ptr + 3]]);
        if proto_handle == 0xFFFF_FFFF { value::encode_null() } else { value::encode_object_handle(proto_handle) }
    }

    fn is_prototype_circular_chain(
        caller: &mut Caller<'_, RuntimeState>,
        target: i64,
        proto: i64,
    ) -> bool {
        let mut current = proto;
        let mut visited = std::collections::HashSet::new();
        while !value::is_null(current) && !value::is_undefined(current) {
            if current == target {
                return true;
            }
            if value::is_proxy(current) {
                let handle = value::decode_proxy_handle(current);
                if !visited.insert(handle) {
                    break;
                }
            } else if value::is_object(current) {
                let handle = value::decode_object_handle(current);
                if !visited.insert(handle) {
                    break;
                }
            } else if value::is_array(current) {
                let handle = value::decode_array_handle(current);
                if !visited.insert(handle) {
                    break;
                }
            }
            current = reflect_get_prototype_of_impl(caller, current);
        }
        false
    }

    fn reflect_set_prototype_of_fn_impl(
        caller: &mut Caller<'_, RuntimeState>,
        target: i64,
        proto: i64,
    ) -> i64 {
        if !is_extensible_impl(caller, target) {
            let current_proto = reflect_get_prototype_of_impl(caller, target);
            return value::encode_bool(current_proto == proto);
        }
        if is_prototype_circular_chain(caller, target, proto) {
            return value::encode_bool(false);
        }
        let Some(ptr) = resolve_handle(caller, target) else { return value::encode_bool(false); };
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else { return value::encode_bool(false); };

        let proto_handle = if value::is_null(proto) {
            0xFFFF_FFFF
        } else if value::is_object(proto) {
            value::decode_object_handle(proto)
        } else if value::is_array(proto) {
            value::decode_array_handle(proto)
        } else if value::is_proxy(proto) {
            value::decode_proxy_handle(proto)
        } else if value::is_function(proto) || value::is_closure(proto) {
            value::decode_object_handle(proto)
        } else {
            0xFFFF_FFFF
        };

        let data = memory.data_mut(&mut *caller);
        if ptr + 4 > data.len() { return value::encode_bool(false); }
        data[ptr..ptr + 4].copy_from_slice(&proto_handle.to_le_bytes());
        value::encode_bool(true)
    }

    let reflect_set_prototype_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, proto: i64| -> i64 {
            if !value::is_object(target) && !value::is_array(target) && !value::is_function(target) && !value::is_proxy(target) {
                set_runtime_error(caller.data(), "TypeError: Reflect.setPrototypeOf called on non-object".to_string());
                return value::encode_bool(false);
            }
            if !value::is_object(proto) && !value::is_null(proto) && !value::is_proxy(proto) && !value::is_array(proto) && !value::is_function(proto) {
                set_runtime_error(caller.data(), "TypeError: Reflect.setPrototypeOf prototype must be an object or null".to_string());
                return value::encode_bool(false);
            }

            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().unwrap(); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'setPrototypeOf' on a proxy that has been revoked".to_string());
                        return value::encode_bool(false);
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "setPrototypeOf").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, proto]);
                            let trap_res = match result {
                                Ok(res) => !value::is_falsy(res),
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: setPrototypeOf trap failed: {}", e));
                                    return value::encode_bool(false);
                                }
                            };
                            if trap_res {
                                let ext = is_extensible_impl(&mut caller, entry.target);
                                if !ext {
                                    let current_proto = reflect_get_prototype_of_impl(&mut caller, entry.target);
                                    if current_proto != proto {
                                        set_runtime_error(caller.data(), "TypeError: Proxy setPrototypeOf invariant violated: target is not extensible and new prototype is different".to_string());
                                        return value::encode_bool(false);
                                    }
                                }
                            }
                            return value::encode_bool(trap_res);
                        }
                    }
                    return reflect_set_prototype_of_fn_impl(&mut caller, entry.target, proto);
                }
            }
            reflect_set_prototype_of_fn_impl(&mut caller, target, proto)
        },
    );

    let reflect_is_extensible_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64| -> i64 {
            if !value::is_object(target) && !value::is_array(target) && !value::is_function(target) && !value::is_proxy(target) {
                set_runtime_error(caller.data(), "TypeError: Reflect.isExtensible called on non-object".to_string());
                return value::encode_bool(false);
            }
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().unwrap();
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'isExtensible' on a proxy that has been revoked".to_string());
                        return value::encode_bool(false);
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "isExtensible")
                            .unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target]);
                            let trap_res = match result {
                                Ok(res) => !value::is_falsy(res),
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: isExtensible trap failed: {}", e));
                                    return value::encode_bool(false);
                                }
                            };
                            let real_res = is_extensible_impl(&mut caller, entry.target);
                            if trap_res != real_res {
                                set_runtime_error(caller.data(), "TypeError: Proxy isExtensible trap returned result that does not match target's extensibility".to_string());
                                return value::encode_bool(false);
                            }
                            return value::encode_bool(trap_res);
                        }
                    }
                    return value::encode_bool(is_extensible_impl(&mut caller, entry.target));
                }
            }
            value::encode_bool(is_extensible_impl(&mut caller, target))
        },
    );

    let reflect_prevent_extensions_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64| -> i64 {
            if !value::is_object(target) && !value::is_array(target) && !value::is_function(target) && !value::is_proxy(target) {
                set_runtime_error(caller.data(), "TypeError: Reflect.preventExtensions called on non-object".to_string());
                return value::encode_bool(false);
            }
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().unwrap();
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'preventExtensions' on a proxy that has been revoked".to_string());
                        return value::encode_bool(false);
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "preventExtensions")
                            .unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target]);
                            let trap_res = match result {
                                Ok(res) => !value::is_falsy(res),
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: preventExtensions trap failed: {}", e));
                                    return value::encode_bool(false);
                                }
                            };
                            if trap_res {
                                let real_res = is_extensible_impl(&mut caller, entry.target);
                                if real_res {
                                    set_runtime_error(caller.data(), "TypeError: Proxy preventExtensions trap returned true, but target remains extensible".to_string());
                                    return value::encode_bool(false);
                                }
                            }
                            return value::encode_bool(trap_res);
                        }
                    }
                    return value::encode_bool(prevent_extensions_impl(&mut caller, entry.target));
                }
            }
            value::encode_bool(prevent_extensions_impl(&mut caller, target))
        },
    );

    let reflect_get_own_property_descriptor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64| -> i64 {
            // Proxy target: trigger getOwnPropertyDescriptor trap
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked { set_runtime_error(caller.data(), "TypeError: Cannot perform 'getOwnPropertyDescriptor' on a proxy that has been revoked".to_string()); return value::encode_undefined(); }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "getOwnPropertyDescriptor").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let trap_result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop]);
                            let descriptor = match trap_result {
                                Ok(desc) => desc,
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: getOwnPropertyDescriptor trap failed: {}", e));
                                    return value::encode_undefined();
                                }
                            };
                            if value::is_undefined(descriptor) {
                                let prop_name = render_value(&mut caller, prop).ok();
                                if let Some(name) = prop_name {
                                    if let Some(name_id) = find_memory_c_string(&mut caller, &name) {
                                        if let Some(t_ptr) = resolve_handle(&mut caller, entry.target) {
                                            if let Some((_, flags, _)) = find_property_slot_by_name_id(&mut caller, t_ptr, name_id) {
                                                let configurable = (flags & constants::FLAG_CONFIGURABLE) != 0;
                                                if !configurable {
                                                    set_runtime_error(caller.data(), "TypeError: Proxy getOwnPropertyDescriptor invariant violated: non-configurable property must not be reported as undefined".to_string());
                                                    return value::encode_undefined();
                                                }
                                            } else if !is_extensible_impl(&mut caller, entry.target) {
                                                set_runtime_error(caller.data(), "TypeError: Proxy getOwnPropertyDescriptor invariant violated: target is non-extensible and property exists".to_string());
                                                return value::encode_undefined();
                                            }
                                        }
                                    }
                                }
                            }
                            return descriptor;
                        }
                    }
                    return reflect_get_own_property_descriptor_impl(&mut caller, entry.target, prop);
                }
                return value::encode_undefined();
            }
            reflect_get_own_property_descriptor_impl(&mut caller, target, prop)
        },
    );
    fn reflect_get_own_property_descriptor_impl(caller: &mut Caller<'_, RuntimeState>, target: i64, prop: i64) -> i64 {
        let prop_name = match render_value(caller, prop) {
            Ok(name) => name,
            Err(_) => return value::encode_undefined(),
        };
        let Some(ptr) = resolve_handle(caller, target) else {
            return value::encode_undefined();
        };
        let Some(name_id) = find_memory_c_string(caller, &prop_name) else {
            return value::encode_undefined();
        };
        let Some((slot_offset, flags, val)) = find_property_slot_by_name_id(caller, ptr, name_id) else {
            return value::encode_undefined();
        };
        let is_accessor = (flags & constants::FLAG_IS_ACCESSOR) != 0;
        let enumerable = (flags & constants::FLAG_ENUMERABLE) != 0;
        let configurable = (flags & constants::FLAG_CONFIGURABLE) != 0;
        let (getter_val, setter_val) = if is_accessor {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return value::encode_undefined();
            };
            let data = memory.data(&*caller);
            if slot_offset + 32 > data.len() { return value::encode_undefined(); }
            let g = i64::from_le_bytes([
                data[slot_offset + 16], data[slot_offset + 17], data[slot_offset + 18], data[slot_offset + 19],
                data[slot_offset + 20], data[slot_offset + 21], data[slot_offset + 22], data[slot_offset + 23],
            ]);
            let s = i64::from_le_bytes([
                data[slot_offset + 24], data[slot_offset + 25], data[slot_offset + 26], data[slot_offset + 27],
                data[slot_offset + 28], data[slot_offset + 29], data[slot_offset + 30], data[slot_offset + 31],
            ]);
            (g, s)
        } else {
            (value::encode_undefined(), value::encode_undefined())
        };
        let desc = alloc_host_object_from_caller(caller, 4);
        if is_accessor {
            let _ = define_host_data_property_from_caller(caller, desc, "get", getter_val);
            let _ = define_host_data_property_from_caller(caller, desc, "set", setter_val);
        } else {
            let _ = define_host_data_property_from_caller(caller, desc, "value", val);
            let _ = define_host_data_property_from_caller(caller, desc, "writable", value::encode_bool((flags & constants::FLAG_WRITABLE) != 0));
        }
        let _ = define_host_data_property_from_caller(caller, desc, "enumerable", value::encode_bool(enumerable));
        let _ = define_host_data_property_from_caller(caller, desc, "configurable", value::encode_bool(configurable));
        desc
    }

    let reflect_define_property_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64, descriptor: i64| -> i64 {
            match define_property_internal(&mut caller, target, prop, descriptor) {
                Ok(success) => value::encode_bool(success),
                Err(e) => {
                    set_runtime_error(caller.data(), e);
                    value::encode_bool(false)
                }
            }
        },
    );

    fn reflect_own_keys_impl(caller: &mut Caller<'_, RuntimeState>, target: i64) -> i64 {
        let Some(ptr) = resolve_handle(caller, target) else { return value::encode_undefined(); };
        let names = collect_own_property_names(caller, ptr, false);
        let arr = alloc_array(caller, names.len() as u32);
        for (i, name) in names.into_iter().enumerate() {
            let name_val = store_runtime_string(caller, name);
            let _ = define_host_data_property_from_caller(caller, arr, &i.to_string(), name_val);
        }
        arr
    }

    let reflect_own_keys_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64| -> i64 {
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'ownKeys' on a proxy that has been revoked".to_string());
                        return value::encode_undefined();
                    }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "ownKeys")
                            .unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let trap_res = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target]);
                            let keys_val = match trap_res {
                                Ok(res) => res,
                                Err(e) => {
                                    set_runtime_error(caller.data(), format!("TypeError: Proxy ownKeys trap failed: {}", e));
                                    return value::encode_undefined();
                                }
                            };
                            let keys = match extract_array_like_elements(&mut caller, keys_val) {
                                Ok(arr) => arr,
                                Err(err) => {
                                    set_runtime_error(caller.data(), err);
                                    return value::encode_undefined();
                                }
                            };

                            // Invariant checks
                            let ext = is_extensible_impl(&mut caller, entry.target);
                            let Some(t_ptr) = resolve_handle(&mut caller, entry.target) else { return value::encode_undefined(); };
                            let target_keys = collect_own_property_names(&mut caller, t_ptr, false);
                            let mut trap_keys_str = Vec::new();
                            for &k in &keys {
                                // 跳过 Symbol 键（不出现在 collect_own_property_names 的结果中）
                                if value::is_symbol(k) { continue; }
                                if let Ok(k_str) = render_value(&mut caller, k) {
                                    trap_keys_str.push(k_str);
                                }
                            }

                            if !ext {
                                let mut match_all = true;
                                for tk in &target_keys {
                                    if !trap_keys_str.contains(tk) {
                                        match_all = false;
                                        break;
                                    }
                                }
                                if !match_all || trap_keys_str.len() != target_keys.len() {
                                    set_runtime_error(caller.data(), "TypeError: Proxy ownKeys invariant violated: target is non-extensible and keys do not match target keys".to_string());
                                    return value::encode_undefined();
                                }
                            } else {
                                for tk in &target_keys {
                                    if let Some(tk_c) = find_memory_c_string(&mut caller, tk) {
                                        if let Some((_, flags, _)) = find_property_slot_by_name_id(&mut caller, t_ptr, tk_c) {
                                            let configurable = (flags & constants::FLAG_CONFIGURABLE) != 0;
                                            if !configurable && !trap_keys_str.contains(tk) {
                                                set_runtime_error(caller.data(), format!("TypeError: Proxy ownKeys invariant violated: non-configurable property '{}' is missing in trap result", tk));
                                                return value::encode_undefined();
                                            }
                                        }
                                    }
                                }
                            }

                            let arr = alloc_array(&mut caller, keys.len() as u32);
                            for (i, &key) in keys.iter().enumerate() {
                                set_array_elem(&mut caller, arr, i as i32, key);
                            }
                            return arr;
                        }
                    }
                    return reflect_own_keys_impl(&mut caller, entry.target);
                }
            }
            reflect_own_keys_impl(&mut caller, target)
        },
    );
    // ── Math builtins ────────────────────────────────────────────────────────
    vec![
        promise_create_fn.into(),                  // 116
        promise_instance_resolve_fn.into(),        // 117
        promise_instance_reject_fn.into(),         // 118
        promise_then_fn.into(),                    // 119
        promise_catch_fn.into(),                   // 120
        promise_finally_fn.into(),                 // 121
        promise_all_fn.into(),                     // 122
        promise_race_fn.into(),                    // 123
        promise_all_settled_fn.into(),             // 124
        promise_any_fn.into(),                     // 125
        promise_resolve_static_fn.into(),          // 126
        promise_reject_static_fn.into(),           // 127
        is_promise_fn.into(),                      // 128
        queue_microtask_fn.into(),                 // 129
        drain_microtasks_fn.into(),                // 130
        async_function_start_fn.into(),            // 131
        async_function_resume_fn.into(),           // 132
        async_function_suspend_fn.into(),          // 133
        continuation_create_fn.into(),             // 134
        continuation_save_var_fn.into(),           // 135
        continuation_load_var_fn.into(),           // 136
        async_generator_start_fn.into(),           // 137
        async_generator_next_fn.into(),            // 138
        async_generator_return_fn.into(),          // 139
        async_generator_throw_fn.into(),           // 140
        native_call_fn.into(),                     // 141
        promise_create_resolve_function_fn.into(), // 142
        promise_create_reject_function_fn.into(),  // 143
        is_callable_fn.into(),                     // 144
        promise_with_resolvers_fn.into(),          // 145
        register_module_namespace_fn.into(),       // 146
        dynamic_import_fn.into(),                  // 147
        eval_direct_fn.into(),                     // 148
        eval_indirect_fn.into(),                   // 149
        jsx_create_element_fn.into(),               // 150
        proxy_create_fn.into(),                     // 151
        proxy_revocable_fn.into(),                  // 152
        reflect_get_fn.into(),                      // 153
        reflect_set_fn.into(),                      // 154
        reflect_has_fn.into(),                      // 155
        reflect_delete_property_fn.into(),           // 156
        reflect_apply_fn.into(),                    // 157
        reflect_construct_fn.into(),                // 158
        reflect_get_prototype_of_fn.into(),          // 159
        reflect_set_prototype_of_fn.into(),          // 160
        reflect_is_extensible_fn.into(),             // 161
        reflect_prevent_extensions_fn.into(),         // 162
        reflect_get_own_property_descriptor_fn.into(), // 163
        reflect_define_property_fn.into(),            // 164
        reflect_own_keys_fn.into(),                  // 165
    ]
}
