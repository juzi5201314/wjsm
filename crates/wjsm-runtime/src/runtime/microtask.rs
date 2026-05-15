use std::sync::Arc;

use wasmtime::{Caller, Extern, Global, Memory, Store, Table, Val};

use crate::types::*;
use crate::runtime::string_utils::{store_runtime_string, store_runtime_string_in_state, read_value_string_bytes};
use crate::runtime::memory::*;
use crate::runtime::object_ops::read_object_property_by_name;
use crate::runtime::render::render_value;
use crate::runtime::promise_core::*;
use crate::runtime::eval::{settle_promise, resolve_promise_from_caller, resolve_promise_from_store, runtime_error_value, passive_reaction_settlement, create_promise_resolving_functions, handle_combinator_reaction_from_caller, handle_combinator_reaction_from_store};
use wjsm_ir::value;

pub(crate) fn set_runtime_error(state: &RuntimeState, message: String) {
    let mut error_lock = state.runtime_error.lock().expect("runtime_error mutex");
    if error_lock.is_none() {
        *error_lock = Some(message);
    }
}

pub(crate) fn drain_microtasks_from_caller(caller: &mut Caller<'_, RuntimeState>, func_table: &Table) {
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

