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
        |_caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            value::encode_bool(value::is_callable(val))
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
            // Update new.target for this call context
            caller.data().new_target.set(value::encode_undefined());
            // Proxy apply trap: if the callable is a proxy, handle via handler.apply
            if value::is_proxy(callable) {
                let handle = value::decode_proxy_handle(callable) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform 'apply' on a proxy that has been revoked".to_string());
                        return value::encode_undefined();
                    }
                    // Look up apply trap on handler
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "apply")
                            .unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            // Create args array from shadow stack
                            let arr = alloc_array(&mut caller, args_count as u32);
                            for i in 0..args_count {
                                let arg = read_shadow_arg(&mut caller, args_base, i as u32);
                                set_array_elem(&mut caller, arr, i, arg);
                            }
                            // Call trap via resolve_and_call
                            let memory = caller.get_export("memory").and_then(|e| e.into_memory()).unwrap();
                            let shadow_sp_global = caller.get_export("__shadow_sp").and_then(|e| e.into_global()).unwrap();
                            let saved_sp = shadow_sp_global.get(&mut caller).i32().unwrap();
                            let trap_args = [entry.target, this_val, arr];
                            let total_size = (trap_args.len() * 8) as i32;
                            for (i, &arg) in trap_args.iter().enumerate() {
                                memory.write(&mut caller, (saved_sp + i as i32 * 8) as usize, &arg.to_le_bytes()).unwrap();
                            }
                            shadow_sp_global.set(&mut caller, Val::I32(saved_sp + total_size)).unwrap();
                            let result = resolve_and_call(&mut caller, trap, entry.handler, saved_sp, trap_args.len() as i32);
                            shadow_sp_global.set(&mut caller, Val::I32(saved_sp)).unwrap();
                            return result;
                        }
                    }
                    // No apply trap, forward to target
                    return resolve_and_call(&mut caller, entry.target, this_val, args_base, args_count);
                }
                return value::encode_undefined();
            }
            let args = (0..args_count.max(0))
                .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                .collect();
            call_native_callable_with_args_from_caller(&mut caller, callable, this_val, args)
                .unwrap_or_else(value::encode_undefined)
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
            if !value::is_object(target) && !value::is_function(target) && !value::is_array(target) && !value::is_proxy(target) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Proxy target must be an object".to_string());
                return value::encode_undefined();
            }
            if !value::is_object(handler) && !value::is_function(handler) && !value::is_array(handler) && !value::is_proxy(handler) {
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
            let _ = receiver;
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
                            return call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop, target])
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

    fn reflect_get_impl(caller: &mut Caller<'_, RuntimeState>, target: i64, prop: i64) -> i64 {
        let obj_ptr = resolve_handle(caller, target);
        if let Some(ptr) = obj_ptr
            && let Ok(prop_name) = render_value(caller, prop)
                && let Some(val) = read_object_property_by_name(caller, ptr, &prop_name) {
                    return val;
                }
        value::encode_undefined()
    }

    let proxy_revocable_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, handler: i64| -> i64 {
            if !value::is_object(target) && !value::is_function(target) && !value::is_array(target) && !value::is_proxy(target) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Proxy target must be an object".to_string());
                return value::encode_undefined();
            }
            if !value::is_object(handler) && !value::is_function(handler) && !value::is_array(handler) && !value::is_proxy(handler) {
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
        |mut caller: Caller<'_, RuntimeState>, target: i64, prop: i64, val: i64, _receiver: i64| -> i64 {
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked { set_runtime_error(caller.data(), "TypeError: Cannot perform 'set' on a proxy that has been revoked".to_string()); return value::encode_bool(false); }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "set").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop, val, target]).unwrap_or_else(|_| value::encode_bool(false));
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
        if let Ok(prop_name) = render_value(caller, prop) {
            let _ = define_host_data_property_from_caller(caller, target, &prop_name, val);
            return value::encode_bool(true);
        }
        value::encode_bool(false)
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

    let reflect_apply_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, this_arg: i64, _args: i64| -> i64 {
            resolve_and_call(&mut caller, target, this_arg, 0, 0)
        },
    );
    let reflect_construct_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, args_array: i64, _new_target: i64| -> i64 {
            let is_callable = value::is_function(target) || value::is_closure(target) || value::is_proxy(target);
            if !is_callable {
                return alloc_host_object_from_caller(&mut caller, 4);
            }
            let this_obj = alloc_host_object_from_caller(&mut caller, 4);
            // Read args from array and push to shadow stack
            let arg_count = if value::is_array(args_array) {
                let handle = value::decode_array_handle(args_array) as usize;
                let Some(ptr) = resolve_handle_idx(&mut caller, handle) else { return this_obj; };
                // Read shadow stack position first (mutable borrow)
                let shadow_sp_global = caller.get_export("__shadow_sp").and_then(|e| e.into_global()).unwrap();
                let saved_sp = shadow_sp_global.get(&mut caller).i32().unwrap();
                // Read array length (immutable borrow on memory)
                let len = {
                    let Some(Extern::Memory(memory)) = caller.get_export("memory") else { return this_obj; };
                    let data = memory.data(&caller);
                    if ptr + 12 > data.len() { return this_obj; };
                    u32::from_le_bytes([data[ptr + 8], data[ptr + 9], data[ptr + 10], data[ptr + 11]]) as usize
                };
                let memory = caller.get_export("memory").and_then(|e| e.into_memory()).unwrap();
                for i in 0..len {
                    let mut buf = [0u8; 8];
                    let _ = memory.read(&mut caller, ptr + 16 + i * 8, &mut buf);
                    let arg_val = i64::from_le_bytes(buf);
                    memory.write(&mut caller, (saved_sp + i as i32 * 8) as usize, &arg_val.to_le_bytes()).unwrap();
                }
                shadow_sp_global.set(&mut caller, Val::I32(saved_sp + len as i32 * 8)).unwrap();
                let result = resolve_and_call(&mut caller, target, this_obj, saved_sp, len as i32);
                shadow_sp_global.set(&mut caller, Val::I32(saved_sp)).unwrap();
                return result;
            } else {
                0
            };
            resolve_and_call(&mut caller, target, this_obj, 0, arg_count as i32)
        },
    );

    let reflect_get_prototype_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64| -> i64 {
            if value::is_proxy(target) {
                let handle = value::decode_proxy_handle(target) as usize;
                let entry = { let table = caller.data().proxy_table.lock().expect("proxy_table mutex"); table.get(handle).cloned() };
                if let Some(entry) = entry {
                    if entry.revoked { set_runtime_error(caller.data(), "TypeError: Cannot perform 'getPrototypeOf' on a proxy that has been revoked".to_string()); return value::encode_undefined(); }
                    if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                        let trap = read_object_property_by_name(&mut caller, handler_ptr, "getPrototypeOf").unwrap_or_else(value::encode_undefined);
                        if !value::is_undefined(trap) && !value::is_null(trap) {
                            return call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target]).unwrap_or_else(|_| value::encode_null());
                        }
                    }
                    return reflect_get_prototype_of_impl(&mut caller, entry.target);
                }
            }
            reflect_get_prototype_of_impl(&mut caller, target)
        },
    );

    fn reflect_get_prototype_of_impl(caller: &mut Caller<'_, RuntimeState>, target: i64) -> i64 {
        let Some(ptr) = resolve_handle(caller, target) else { return value::encode_undefined(); };
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else { return value::encode_null(); };
        let data = memory.data(&*caller);
        if ptr + 4 > data.len() { return value::encode_null(); }
        let proto_handle = u32::from_le_bytes([data[ptr], data[ptr + 1], data[ptr + 2], data[ptr + 3]]);
        if proto_handle == 0xFFFF_FFFF { value::encode_null() } else { value::encode_object_handle(proto_handle) }
    }

    let reflect_set_prototype_of_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _target: i64, _proto: i64| -> i64 {
            value::encode_bool(true)
        },
    );

    let reflect_is_extensible_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _target: i64| -> i64 {
            value::encode_bool(true)
        },
    );

    let reflect_prevent_extensions_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _target: i64| -> i64 {
            value::encode_bool(true)
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
                            return call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, prop])
                                .unwrap_or_else(|_| value::encode_undefined());
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
            let value_val = resolve_handle(&mut caller, descriptor)
                .and_then(|p| read_object_property_by_name(&mut caller, p, "value"))
                .unwrap_or_else(value::encode_undefined);
            reflect_set_impl(&mut caller, target, prop, value_val)
        },
    );

    let reflect_own_keys_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64| -> i64 {
            let Some(ptr) = resolve_handle(&mut caller, target) else { return value::encode_undefined(); };
            let names = collect_own_property_names(&mut caller, ptr, false);
            let arr = alloc_array(&mut caller, names.len() as u32);
            for (i, name) in names.into_iter().enumerate() {
                let name_val = store_runtime_string(&caller, name);
                let _ = define_host_data_property_from_caller(&mut caller, arr, &i.to_string(), name_val);
            }
            arr
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
