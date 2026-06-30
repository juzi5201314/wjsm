//! Async `func_wrap_async` overrides for host imports that re-enter Wasm (misc, array callbacks, json_parse).

use anyhow::Result;
use std::sync::atomic::Ordering;
use wasmtime::{Caller, Linker, Store};

use crate::*;

use super::proxy_traps::{
    proxy_trap_handler_trap, proxy_trap_property_key_value, proxy_trap_proxy_entry,
};

fn type_error_exception_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    msg: &'static str,
) -> i64 {
    // 统一委托给共享实现，保持 TAG_EXCEPTION 构造逻辑单一来源。
    make_type_error_exception(caller, msg)
}

pub(crate) async fn native_call_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    callable: i64,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let new_target_val = caller.data().new_target.load(Ordering::Relaxed);
    caller
        .data()
        .new_target
        .store(value::encode_undefined(), Ordering::Relaxed);

    if value::is_proxy(callable) {
        let handle = value::decode_proxy_handle(callable) as usize;
        let entry = {
            let table = caller
                .data()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                return type_error_exception_from_caller(
                    caller,
                    "TypeError: Cannot perform call on a proxy that has been revoked",
                );
            }

            if !value::is_undefined(new_target_val) {
                if !is_constructor_in_runtime(caller, entry.target) {
                    return type_error_exception_from_caller(
                        caller,
                        "TypeError: Proxy target must be a constructor",
                    );
                }
                if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                    let trap = read_object_property_by_name(caller, handler_ptr, "construct")
                        .unwrap_or_else(value::encode_undefined);
                    if !value::is_undefined(trap) && !value::is_null(trap) {
                        let arr = alloc_array(caller, args_count as u32);
                        for i in 0..args_count {
                            let arg = read_shadow_arg(caller, args_base, i as u32);
                            set_array_elem(caller, arr, i, arg);
                        }
                        let trap_res = call_wasm_callback_async(
                            caller,
                            trap,
                            entry.handler,
                            &[entry.target, arr, new_target_val],
                        )
                        .await;
                        return match trap_res {
                            Ok(res) => {
                                if !value::is_js_object(res) {
                                    type_error_exception_from_caller(
                                        caller,
                                        "TypeError: Proxy construct trap returned non-object",
                                    )
                                } else {
                                    res
                                }
                            }
                            Err(_) => type_error_exception_from_caller(
                                caller,
                                "TypeError: Proxy construct trap failed",
                            ),
                        };
                    }
                }
                caller
                    .data()
                    .new_target
                    .store(new_target_val, Ordering::Relaxed);
                let result =
                    resolve_and_call_async(caller, entry.target, this_val, args_base, args_count)
                        .await;
                caller
                    .data()
                    .new_target
                    .store(value::encode_undefined(), Ordering::Relaxed);
                return result;
            }

            if !is_callable_in_runtime(caller, entry.target) {
                return type_error_exception_from_caller(
                    caller,
                    "TypeError: Proxy target must be callable",
                );
            }
            if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                let trap = read_object_property_by_name(caller, handler_ptr, "apply")
                    .unwrap_or_else(value::encode_undefined);
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    let arr = alloc_array(caller, args_count as u32);
                    for i in 0..args_count {
                        let arg = read_shadow_arg(caller, args_base, i as u32);
                        set_array_elem(caller, arr, i, arg);
                    }
                    let result = call_wasm_callback_async(
                        caller,
                        trap,
                        entry.handler,
                        &[entry.target, this_val, arr],
                    )
                    .await;
                    return result.unwrap_or_else(|_| {
                        set_runtime_error(
                            caller.data(),
                            "TypeError: Proxy apply trap failed".to_string(),
                        );
                        value::encode_undefined()
                    });
                }
            }
            return resolve_and_call_async(caller, entry.target, this_val, args_base, args_count)
                .await;
        }
        return value::encode_undefined();
    }
    if value::is_bound(callable) {
        return resolve_and_call_async(caller, callable, this_val, args_base, args_count).await;
    }

    if value::is_native_callable(callable) {
        if !value::is_undefined(new_target_val) {
            caller
                .data()
                .new_target
                .store(new_target_val, Ordering::Relaxed);
        }
        let args = (0..args_count.max(0))
            .map(|index| read_shadow_arg(caller, args_base, index as u32))
            .collect();
        let result =
            call_native_callable_with_args_from_caller_async(caller, callable, this_val, args)
                .await
                .unwrap_or_else(value::encode_undefined);
        caller
            .data()
            .new_target
            .store(value::encode_undefined(), Ordering::Relaxed);
        return result;
    }

    resolve_callable_and_call_async(caller, callable, this_val, args_base, args_count).await
}

pub(crate) fn define_misc_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "drain_microtasks",
        |mut caller: Caller<'_, RuntimeState>, (): ()| {
            Box::new(async move {
                let table = caller.get_export("__table").and_then(|e| e.into_table());
                let Some(func_table) = table else {
                    return;
                };
                drain_microtasks_from_caller_async(&mut caller, &func_table).await;
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "native_call",
        |mut caller: Caller<'_, RuntimeState>,
         (callable, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                native_call_from_caller_async(
                    &mut caller,
                    callable,
                    this_val,
                    args_base,
                    args_count,
                )
                .await
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "eval_direct",
        |mut caller: Caller<'_, RuntimeState>, (code, scope_env): (i64, i64)| {
            Box::new(async move {
                perform_eval_from_caller_async(&mut caller, code, Some(scope_env)).await
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "eval_indirect",
        |mut caller: Caller<'_, RuntimeState>, (code,): (i64,)| {
            Box::new(async move { perform_eval_from_caller_async(&mut caller, code, None).await })
        },
    )?;

    Ok(())
}

pub(crate) fn define_timers_arrays_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    fn delay_ms_from(delay: i64) -> u64 {
        let delay_f64 = if value::is_f64(delay) {
            value::decode_f64(delay)
        } else {
            f64::NAN
        };
        if delay_f64.is_nan() || delay_f64.is_sign_negative() {
            0
        } else if delay_f64 > (u32::MAX as f64) {
            u32::MAX as u64
        } else {
            delay_f64 as u64
        }
    }

    fn timer_callback_type_error(caller: &mut Caller<'_, RuntimeState>) -> i64 {
        let msg = "TypeError: timer callback must be callable";
        let msg_val = store_runtime_string(caller, msg.to_string());
        let error_obj = create_error_object(caller, "TypeError", msg_val, value::encode_undefined());
        let mut errors = caller
            .data()
            .error_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let idx = errors.len() as u32;
        errors.push(crate::ErrorEntry {
            name: "TypeError".to_string(),
            message: msg.to_string(),
            value: error_obj,
        });
        value::encode_handle(value::TAG_EXCEPTION, idx)
    }
    linker.func_wrap_async(
        "env",
        "set_timeout",
        |mut caller: Caller<'_, RuntimeState>, (callback, delay): (i64, i64)| {
            Box::new(async move {
                if !is_callable_in_runtime(&mut caller, callback) {
                    return timer_callback_type_error(&mut caller);
                }
                let delay_ms = delay_ms_from(delay);
                let id = {
                    let mut next_id = caller
                        .data()
                        .next_timer_id
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let id = *next_id;
                    *next_id += 1;
                    id
                };
                let deadline = Instant::now() + Duration::from_millis(delay_ms);
                let mut timers = caller
                    .data()
                    .timers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                timers.push(TimerEntry {
                    id,
                    deadline,
                    callback,
                    repeating: false,
                    interval: Duration::from_millis(delay_ms),
                });
                value::encode_f64(id as f64)
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "clear_timeout",
        |caller: Caller<'_, RuntimeState>, (timer_id,): (i64,)| {
            Box::new(async move {
                if value::is_f64(timer_id) {
                    let id = value::decode_f64(timer_id) as u32;
                    caller
                        .data()
                        .cancelled_timers
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(id);
                }
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "set_interval",
        |mut caller: Caller<'_, RuntimeState>, (callback, delay): (i64, i64)| {
            Box::new(async move {
                if !is_callable_in_runtime(&mut caller, callback) {
                    return timer_callback_type_error(&mut caller);
                }
                let delay_ms = delay_ms_from(delay);
                let id = {
                    let mut next_id = caller
                        .data()
                        .next_timer_id
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let id = *next_id;
                    *next_id += 1;
                    id
                };
                let deadline = Instant::now() + Duration::from_millis(delay_ms);
                let mut timers = caller
                    .data()
                    .timers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                timers.push(TimerEntry {
                    id,
                    deadline,
                    callback,
                    repeating: true,
                    interval: Duration::from_millis(delay_ms),
                });
                value::encode_f64(id as f64)
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "clear_interval",
        |caller: Caller<'_, RuntimeState>, (timer_id,): (i64,)| {
            Box::new(async move {
                if value::is_f64(timer_id) {
                    let id = value::decode_f64(timer_id) as u32;
                    caller
                        .data()
                        .cancelled_timers
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(id);
                }
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "json_parse",
        |mut caller: Caller<'_, RuntimeState>, (val, reviver): (i64, i64)| {
            Box::new(async move { json_parse_to_wasm_async(&mut caller, val, reviver).await })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "json_stringify",
        |mut caller: Caller<'_, RuntimeState>, (val, replacer, space): (i64, i64, i64)| {
            Box::new(async move {
                runtime_json_stringify_full_async(&mut caller, val, replacer, space).await
            })
        },
    )?;
    Ok(())
}

mod reentrant_array_async;
mod reentrant_proxy_async;
mod reentrant_string_async;
mod reentrant_typedarray_async;

pub(crate) use reentrant_array_async::*;
pub(crate) use reentrant_proxy_async::*;
pub(crate) use reentrant_string_async::*;
pub(crate) use reentrant_typedarray_async::*;
