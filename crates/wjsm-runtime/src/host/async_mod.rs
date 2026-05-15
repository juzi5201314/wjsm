use wasmtime::*;
use wjsm_ir::value;
use std::sync::Arc;
use std::sync::Mutex;

use crate::types::*;
use crate::runtime::*;
use crate::host::promise;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let is_callable_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            value::encode_bool(value::is_callable(val))
        },
    );

    // ── Import 129: queue_microtask(i64) -> () ──────────────────────────────

    let queue_microtask_fn = Func::wrap(
        &mut *store,
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

    let drain_microtasks_fn = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>| {
        let table = caller.get_export("__table").and_then(|e| e.into_table());
        let Some(func_table) = table else { return };
        drain_microtasks_from_caller(&mut caller, &func_table);
    });

    // ── Import 131: async_function_start(i64) -> i64 ────────────────────────

    let async_function_start_fn = Func::wrap(
        &mut *store,
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
            let outer_promise = promise::alloc_promise(&mut caller, PromiseEntry::pending());

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
        &mut *store,
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
        &mut *store,
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
                        return;
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
                        return;
                    }
                }
            }
        },
    );

    // ── Import 134: continuation_create(i64, i64, i64) -> i64 ───────────────

    let continuation_create_fn = Func::wrap(
        &mut *store,
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
        &mut *store,
        |caller: Caller<'_, RuntimeState>, continuation: i64, slot: i64, val: i64| {
            let handle = value::decode_object_handle(continuation) as usize;
            let actual_slot = nanbox_to_usize(slot);
            let mut table = caller
                .data()
                .continuation_table
                .lock()
                .expect("continuation table mutex");
            if let Some(entry) = table.get_mut(handle) {
                if actual_slot < entry.captured_vars.len() {
                    entry.captured_vars[actual_slot] = val;
                }
            }
        },
    );

    // ── Import 136: continuation_load_var(i64, i64) -> i64 ──────────────────

    let continuation_load_var_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, continuation: i64, slot: i64| -> i64 {
            let handle = value::decode_object_handle(continuation) as usize;
            let actual_slot = nanbox_to_usize(slot);
            let table = caller
                .data()
                .continuation_table
                .lock()
                .expect("continuation table mutex");
            if let Some(entry) = table.get(handle) {
                if actual_slot < entry.captured_vars.len() {
                    return entry.captured_vars[actual_slot];
                }
            }
            value::encode_undefined()
        },
    );

    // ── Import 137: async_generator_start(i64) -> i64 ───────────────────────

    let async_generator_start_fn = Func::wrap(
        &mut *store,
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
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            let resume_promise = promise::alloc_promise(&mut caller, PromiseEntry::pending());
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
                let active = entry.active_request.take();
                active
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
        &mut *store,
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
        &mut *store,
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

    let register_module_namespace_fn = Func::wrap(
        &mut *store,
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
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, module_id: i64| -> i64 {
            let mid = module_id as u32;

            // 创建 Promise 并添加 .then/.catch/.finally 方法
            let promise = promise::alloc_promise(&mut caller, PromiseEntry::pending());
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

    vec![
        (129, queue_microtask_fn),
        (130, drain_microtasks_fn),
        (131, async_function_start_fn),
        (132, async_function_resume_fn),
        (133, async_function_suspend_fn),
        (134, continuation_create_fn),
        (135, continuation_save_var_fn),
        (136, continuation_load_var_fn),
        (137, async_generator_start_fn),
        (138, async_generator_next_fn),
        (139, async_generator_return_fn),
        (140, async_generator_throw_fn),
        (144, is_callable_fn),
        (146, register_module_namespace_fn),
        (147, dynamic_import_fn),
    ]
}
