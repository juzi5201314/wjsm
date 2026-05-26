use crate::*;

pub(crate) fn register_promise_imports(mut store: &mut Store<RuntimeState>) -> Vec<Extern> {
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
                    clear_pending_unhandled_rejection(caller.data(), handle);
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
                    clear_pending_unhandled_rejection(caller.data(), handle);
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
                    clear_pending_unhandled_rejection(caller.data(), handle);
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
            let obj = { let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv"); alloc_host_object(&mut caller, &_wjsm_env, 3) };
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

    vec![
        promise_create_fn.into(),             // 116
        promise_instance_resolve_fn.into(),   // 117
        promise_instance_reject_fn.into(),    // 118
        promise_then_fn.into(),               // 119
        promise_catch_fn.into(),              // 120
        promise_finally_fn.into(),            // 121
        promise_resolve_static_fn.into(),     // 126
        promise_reject_static_fn.into(),      // 127
        is_promise_fn.into(),                 // 128
        promise_create_resolve_function_fn.into(), // 142
        promise_create_reject_function_fn.into(),  // 143
        promise_with_resolvers_fn.into(),     // 145
    ]
}
