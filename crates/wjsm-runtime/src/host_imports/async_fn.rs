use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::*;

pub(crate) fn define_async_fn(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let async_function_start_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, fn_table_idx: i64| -> i64 {
            let fn_table_idx = if value::is_function(fn_table_idx) {
                value::decode_function_idx(fn_table_idx)
            } else if value::is_closure(fn_table_idx) {
                let idx = value::decode_closure_idx(fn_table_idx);
                let closures = caller
                    .data()
                    .closures
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                closures.get(idx as usize).map(|e| e.func_idx).unwrap_or(0)
            } else {
                nanbox_to_u32(fn_table_idx)
            };
            let outer_promise = alloc_promise(&mut caller, PromiseEntry::pending());

            let cont_handle = crate::runtime_async_fn::alloc_continuation_handle(
                caller.data(),
                fn_table_idx,
                outer_promise,
                4,
            );
            {
                let mut c_table = caller
                    .data()
                    .continuation_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = c_table.get_mut(cont_handle as usize) {
                    entry.captured_vars[2] = outer_promise;
                }
            }

            let mut queue = caller
                .data()
                .microtask_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            queue.push_back(Microtask::AsyncResume {
                fn_table_idx,
                continuation: value::encode_object_handle(cont_handle),
                state: 0,
                resume_val: value::encode_undefined(),
                completion: 0,
            });

            outer_promise
        },
    );
    linker.define(
        &mut store,
        "env",
        "async_function_start",
        async_function_start_fn,
    )?;

    // ── Import 132: async_function_resume(i64, i64, i64, i64, i64) -> () ───
    linker.func_wrap_async(
        "env",
        "async_function_resume",
        |mut caller: Caller<'_, RuntimeState>,
         (fn_table_idx, continuation, state, resume_val, completion_raw): (
            i64,
            i64,
            i64,
            i64,
            i64,
        )| {
            Box::new(async move {
                let resolved_fn_idx = if value::is_function(fn_table_idx) {
                    value::decode_function_idx(fn_table_idx)
                } else if value::is_closure(fn_table_idx) {
                    let idx = value::decode_closure_idx(fn_table_idx);
                    let closures = caller
                        .data()
                        .closures
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    closures.get(idx as usize).map(|e| e.func_idx).unwrap_or(0)
                } else {
                    nanbox_to_u32(fn_table_idx)
                };
                let state = nanbox_to_u32(state);
                let completion = nanbox_to_u32(completion_raw) as u8;
                {
                    let cont_handle = value::decode_object_handle(continuation) as usize;
                    let mut c_table = caller
                        .data()
                        .continuation_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if let Some(entry) = c_table.get_mut(cont_handle) {
                        while entry.captured_vars.len() < 2 {
                            entry.captured_vars.push(value::encode_undefined());
                        }
                        entry.captured_vars[0] = value::encode_f64(state as f64);
                        entry.captured_vars[1] = value::encode_f64(completion as f64);
                    }
                }
                // §27.7.5.2 AsyncFunctionStart: 初始调用(state=0)时同步执行函数体，
                // 直到第一个 await 才挂起；后续恢复仍走微任务队列。
                if state == 0 {
                    let env = WasmEnv::from_caller(&mut caller)
                        .expect("WasmEnv in async_function_resume");
                    let func_ref = env.func_table.get(&mut caller, resolved_fn_idx as u64);
                    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
                    if let Some(func) = func {
                        let mut results = [Val::I64(0)];
                        let _ = func
                            .call_async(
                                &mut caller,
                                &[
                                    Val::I64(continuation),
                                    Val::I64(resume_val),
                                    Val::I32(0),
                                    Val::I32(0),
                                ],
                                &mut results,
                            )
                            .await;
                        let cont_handle = value::decode_object_handle(continuation) as usize;
                        let outer_promise = {
                            let c_table = caller
                                .data()
                                .continuation_table
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            c_table.get(cont_handle).map(|e| e.outer_promise)
                        };
                        if let Some(outer_promise) = outer_promise {
                            if is_promise_settled(caller.data(), outer_promise) {
                                let mut c_table = caller
                                    .data()
                                    .continuation_table
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                if let Some(entry) = c_table.get_mut(cont_handle) {
                                    entry.completed = true;
                                }
                            }
                        }
                        return;
                    }
                }
                let mut queue = caller
                    .data()
                    .microtask_queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                queue.push_back(Microtask::AsyncResume {
                    fn_table_idx: resolved_fn_idx,
                    continuation,
                    state,
                    resume_val,
                    completion,
                });
            })
        },
    )?;

    // ── Import 133: async_function_suspend(i64, i64, i64) -> () ─────────────
    let async_function_suspend_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, continuation: i64, awaited_promise: i64, state: i64| {
            let cont_handle = value::decode_object_handle(continuation) as usize;
            let cont_fn_idx = {
                let mut c_table = caller
                    .data()
                    .continuation_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let Some(entry) = c_table.get_mut(cont_handle) else {
                    return;
                };
                while entry.captured_vars.len() < 4 {
                    entry.captured_vars.push(value::encode_undefined());
                }
                entry.captured_vars[0] = value::encode_f64(state as f64);
                entry.captured_vars[1] = value::encode_f64(0.0);
                entry.fn_table_idx
            };

            let awaited_handle = value::decode_object_handle(awaited_promise) as usize;
            let mut p_table = caller
                .data()
                .promise_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = promise_entry_mut(&mut p_table, awaited_handle) {
                // §15.8.1 — await 标记 promise 为已处理
                entry.handled = true;
                clear_pending_unhandled_rejection(caller.data(), awaited_handle);
                match &entry.state {
                    PromiseState::Pending => {
                        entry.fulfill_reactions.push(PromiseReaction::new_async(
                            cont_fn_idx,
                            continuation,
                            ReactionType::Fulfill,
                            state as u32,
                        ));
                        entry.reject_reactions.push(PromiseReaction::new_async(
                            cont_fn_idx,
                            continuation,
                            ReactionType::Reject,
                            state as u32,
                        ));
                    }
                    PromiseState::Fulfilled(val) => {
                        let val = *val;
                        let reactions = vec![PromiseReaction::new_async(
                            cont_fn_idx,
                            continuation,
                            ReactionType::Fulfill,
                            state as u32,
                        )];
                        drop(p_table);
                        queue_promise_reactions(caller.data(), reactions, val, false);
                    }
                    PromiseState::Rejected(reason) => {
                        let reason = *reason;
                        let reactions = vec![PromiseReaction::new_async(
                            cont_fn_idx,
                            continuation,
                            ReactionType::Reject,
                            state as u32,
                        )];
                        drop(p_table);
                        queue_promise_reactions(caller.data(), reactions, reason, true);
                    }
                }
            } else {
                // #165: 防御 — await lowering 总会经 PromiseResolveStatic 把操作数转为原生
                // promise；若因任何不变量破坏使 awaited_promise 不是原生 promise，把值当作
                // 已 fulfill 立即恢复 continuation，避免 async 函数静默挂起。
                drop(p_table);
                let mut queue = caller
                    .data()
                    .microtask_queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                queue.push_back(Microtask::AsyncResume {
                    fn_table_idx: cont_fn_idx,
                    continuation,
                    state: state as u32,
                    resume_val: awaited_promise,
                    completion: 0,
                });
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "async_function_suspend",
        async_function_suspend_fn,
    )?;

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
                let closures = caller
                    .data()
                    .closures
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                closures.get(idx as usize).map(|e| e.func_idx).unwrap_or(0)
            } else {
                nanbox_to_u32(fn_table_idx)
            };
            let total_slots = nanbox_to_usize(captured_var_count);
            let handle = crate::runtime_async_fn::alloc_continuation_handle(
                caller.data(),
                resolved_fn_idx,
                outer_promise,
                total_slots,
            );
            value::encode_object_handle(handle)
        },
    );
    linker.define(
        &mut store,
        "env",
        "continuation_create",
        continuation_create_fn,
    )?;

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
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = table.get_mut(handle)
                && actual_slot < entry.captured_vars.len()
            {
                entry.captured_vars[actual_slot] = val;
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "continuation_save_var",
        continuation_save_var_fn,
    )?;

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
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = table.get(handle)
                && actual_slot < entry.captured_vars.len()
            {
                return entry.captured_vars[actual_slot];
            }
            value::encode_undefined()
        },
    );
    linker.define(
        &mut store,
        "env",
        "continuation_load_var",
        continuation_load_var_fn,
    )?;

    Ok(())
}
