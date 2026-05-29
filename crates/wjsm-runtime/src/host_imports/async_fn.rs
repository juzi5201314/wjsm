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
                completed: false,
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
    linker.define(
        &mut store,
        "env",
        "async_function_start",
        async_function_start_fn,
    )?;

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
    linker.define(
        &mut store,
        "env",
        "async_function_resume",
        async_function_resume_fn,
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
                    .expect("continuation table mutex");
                let Some(entry) = c_table.get_mut(cont_handle) else {
                    return;
                };
                while entry.captured_vars.len() < 4 {
                    entry.captured_vars.push(value::encode_undefined());
                }
                entry.captured_vars[0] = value::encode_f64(state as f64);
                entry.captured_vars[1] = value::encode_bool(false);
                entry.fn_table_idx
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
                completed: false,
            });
            if let Some(entry) = table.get_mut(handle as usize) {
                entry.captured_vars[0] = value::encode_f64(0.0);
                entry.captured_vars[1] = value::encode_bool(false);
            }
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
                .expect("continuation table mutex");
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
                .expect("continuation table mutex");
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
