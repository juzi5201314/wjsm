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
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
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

    if !value::is_undefined(new_target_val) {
        caller
            .data()
            .new_target
            .store(new_target_val, Ordering::Relaxed);
    }
    let args = (0..args_count.max(0))
        .map(|index| read_shadow_arg(caller, args_base, index as u32))
        .collect();
    let result = call_native_callable_with_args_from_caller_async(caller, callable, this_val, args)
        .await
        .unwrap_or_else(value::encode_undefined);
    caller
        .data()
        .new_target
        .store(value::encode_undefined(), Ordering::Relaxed);
    result
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
            f64::from_bits(delay as u64)
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
        let error_obj = create_error_object(caller, "TypeError", msg_val);
        let mut errors = caller.data().error_table.lock().expect("error table mutex");
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
                        .expect("next_timer_id mutex");
                    let id = *next_id;
                    *next_id += 1;
                    id
                };
                let deadline = Instant::now() + Duration::from_millis(delay_ms);
                let mut timers = caller.data().timers.lock().expect("timers mutex");
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
                    let id = f64::from_bits(timer_id as u64) as u32;
                    caller
                        .data()
                        .cancelled_timers
                        .lock()
                        .expect("cancelled_timers mutex")
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
                        .expect("next_timer_id mutex");
                    let id = *next_id;
                    *next_id += 1;
                    id
                };
                let deadline = Instant::now() + Duration::from_millis(delay_ms);
                let mut timers = caller.data().timers.lock().expect("timers mutex");
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
                    let id = f64::from_bits(timer_id as u64) as u32;
                    caller
                        .data()
                        .cancelled_timers
                        .lock()
                        .expect("cancelled_timers mutex")
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

async fn sort_compare_async(
    caller: &mut Caller<'_, RuntimeState>,
    cmp: i64,
    a: i64,
    b: i64,
) -> std::cmp::Ordering {
    let result = call_wasm_callback_async(caller, cmp, value::encode_undefined(), &[a, b])
        .await
        .unwrap_or(value::encode_f64(0.0));
    let v = f64::from_bits(result as u64);
    if v > 0.0 {
        std::cmp::Ordering::Greater
    } else if v < 0.0 {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Equal
    }
}

async fn arr_proto_sort_async_body(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return this_val;
    };
    let len = read_array_length(caller, ptr).unwrap_or(0) as usize;
    if len <= 1 {
        return this_val;
    }
    let mut elems: Vec<i64> = (0..len)
        .map(|i| read_array_elem(caller, ptr, i as u32).unwrap_or(value::encode_undefined()))
        .collect();
    if args_count > 0 && value::is_callable(read_shadow_arg(caller, args_base, 0)) {
        let cmp = read_shadow_arg(caller, args_base, 0);
        for i in 0..elems.len() {
            for j in i + 1..elems.len() {
                if sort_compare_async(caller, cmp, elems[i], elems[j]).await
                    == std::cmp::Ordering::Greater
                {
                    elems.swap(i, j);
                }
            }
        }
    } else {
        let keys: Vec<String> = elems
            .iter()
            .map(|e| render_value(caller, *e).unwrap_or_default())
            .collect();
        let mut indexed: Vec<(usize, &i64)> = (0..len).map(|i| (i, &elems[i])).collect();
        indexed.sort_by(|(ia, _), (ib, _)| {
            let ka = &keys[*ia];
            let kb = &keys[*ib];
            let cmp = ka.cmp(kb);
            if cmp == std::cmp::Ordering::Equal {
                ia.cmp(ib)
            } else {
                cmp
            }
        });
        elems = indexed.iter().map(|(_, e)| **e).collect();
    }
    for (i, &elem) in elems.iter().enumerate() {
        write_array_elem(caller, ptr, i as u32, elem);
    }
    this_val
}

macro_rules! wrap_array_callback_async {
    ($linker:expr, $name:expr, $body:expr) => {
        $linker.func_wrap_async(
            "env",
            $name,
            |mut caller: Caller<'_, RuntimeState>,
             (_env_obj, this_val, args_base, args_count): (i64, i64, i32, i32)| {
                Box::new(async move { $body(&mut caller, this_val, args_base, args_count).await })
            },
        )?;
    };
}

pub(crate) fn define_array_object_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "arr_proto_sort",
        |mut caller: Caller<'_, RuntimeState>,
         (_env_obj, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                arr_proto_sort_async_body(&mut caller, this_val, args_base, args_count).await
            })
        },
    )?;

    wrap_array_callback_async!(linker, "arr_proto_for_each", arr_proto_for_each_async);
    wrap_array_callback_async!(linker, "arr_proto_map", arr_proto_map_async);
    wrap_array_callback_async!(linker, "arr_proto_filter", arr_proto_filter_async);
    wrap_array_callback_async!(linker, "arr_proto_reduce", arr_proto_reduce_async);
    wrap_array_callback_async!(
        linker,
        "arr_proto_reduce_right",
        arr_proto_reduce_right_async
    );
    wrap_array_callback_async!(linker, "arr_proto_find", arr_proto_find_async);
    wrap_array_callback_async!(linker, "arr_proto_find_index", arr_proto_find_index_async);
    wrap_array_callback_async!(linker, "arr_proto_some", arr_proto_some_async);
    wrap_array_callback_async!(linker, "arr_proto_every", arr_proto_every_async);
    wrap_array_callback_async!(linker, "arr_proto_flat_map", arr_proto_flat_map_async);

    linker.func_wrap_async(
        "env",
        "array_push_spread",
        |mut caller: Caller<'_, RuntimeState>, (arr, iterable): (i64, i64)| {
            Box::new(async move {
                super::array_object::array_push_spread_impl_async(&mut caller, arr, iterable).await
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "func_call",
        |mut caller: Caller<'_, RuntimeState>,
         (func, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                resolve_and_call_async(&mut caller, func, this_val, args_base, args_count).await
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "func_apply",
        |mut caller: Caller<'_, RuntimeState>, (func, this_val, args_array): (i64, i64, i64)| {
            Box::new(
                async move { func_apply_impl_async(&mut caller, func, this_val, args_array).await },
            )
        },
    )?;

    Ok(())
}

async fn arr_proto_for_each_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val])
            .await
            .is_err()
        {
            return value::encode_undefined();
        }
    }
    value::encode_undefined()
}

async fn arr_proto_map_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let new_arr = alloc_array(caller, len);
    let Some(new_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        let result = match call_wasm_callback_async(
            caller,
            cb,
            this_arg,
            &[elem, idx_val, this_val],
        )
        .await
        {
            Ok(r) => r,
            Err(_) => value::encode_undefined(),
        };
        write_array_elem(caller, new_ptr, i, result);
    }
    write_array_length(caller, new_ptr, len);
    new_arr
}

async fn arr_proto_filter_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let mut passed: Vec<i64> = Vec::new();
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        let ok = match call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val])
            .await
        {
            Ok(r) => value::is_truthy(r),
            Err(_) => false,
        };
        if ok {
            passed.push(elem);
        }
    }
    let new_arr = alloc_array(caller, passed.len() as u32);
    let Some(new_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    for (i, elem) in passed.iter().enumerate() {
        write_array_elem(caller, new_ptr, i as u32, *elem);
    }
    write_array_length(caller, new_ptr, passed.len() as u32);
    new_arr
}

async fn arr_proto_reduce_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0) as usize;
    if len == 0 {
        if args_count < 2 {
            *caller
                .data()
                .runtime_error
                .lock()
                .expect("runtime error mutex") =
                Some("TypeError: Reduce of empty array with no initial value".to_string());
            return value::encode_undefined();
        }
        return read_shadow_arg(caller, args_base, 1);
    }
    let mut acc: i64;
    let mut start_idx = 0usize;
    if args_count >= 2 {
        acc = read_shadow_arg(caller, args_base, 1);
    } else {
        acc = read_array_elem(caller, ptr, 0).unwrap_or(value::encode_undefined());
        start_idx = 1;
    }
    for i in start_idx..len {
        let elem = read_array_elem(caller, ptr, i as u32).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        match call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[acc, elem, idx_val, this_val],
        )
        .await
        {
            Ok(r) => acc = r,
            Err(_) => return value::encode_undefined(),
        }
    }
    acc
}

async fn arr_proto_reduce_right_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0) as i32;
    if len == 0 {
        if args_count < 2 {
            *caller
                .data()
                .runtime_error
                .lock()
                .expect("runtime error mutex") =
                Some("TypeError: Reduce of empty array with no initial value".to_string());
            return value::encode_undefined();
        }
        return read_shadow_arg(caller, args_base, 1);
    }
    let mut acc: i64;
    let mut start_idx = len - 1;
    if args_count >= 2 {
        acc = read_shadow_arg(caller, args_base, 1);
    } else {
        acc = read_array_elem(caller, ptr, start_idx as u32).unwrap_or(value::encode_undefined());
        start_idx = len - 2;
    }
    for i in (0..=start_idx as usize).rev() {
        let elem = read_array_elem(caller, ptr, i as u32).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        match call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[acc, elem, idx_val, this_val],
        )
        .await
        {
            Ok(r) => acc = r,
            Err(_) => return value::encode_undefined(),
        }
    }
    acc
}

async fn arr_proto_find_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[elem, idx_val, this_val],
        )
        .await
            && value::is_truthy(r)
        {
            return elem;
        }
    }
    value::encode_undefined()
}

async fn arr_proto_find_index_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_f64(-1.0);
    }
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_f64(-1.0);
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[elem, idx_val, this_val],
        )
        .await
            && value::is_truthy(r)
        {
            return value::encode_f64(i as f64);
        }
    }
    value::encode_f64(-1.0)
}

async fn arr_proto_some_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_bool(false);
    }
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_bool(false);
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[elem, idx_val, this_val],
        )
        .await
            && value::is_truthy(r)
        {
            return value::encode_bool(true);
        }
    }
    value::encode_bool(false)
}

async fn arr_proto_every_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_bool(false);
    }
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_bool(false);
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        match call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[elem, idx_val, this_val],
        )
        .await
        {
            Ok(r) => {
                if !value::is_truthy(r) {
                    return value::encode_bool(false);
                }
            }
            Err(_) => return value::encode_bool(false),
        }
    }
    value::encode_bool(true)
}

async fn arr_proto_flat_map_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let mut elements: Vec<i64> = Vec::new();
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        let mapped = match call_wasm_callback_async(
            caller,
            cb,
            this_arg,
            &[elem, idx_val, this_val],
        )
        .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        if value::is_array(mapped) {
            if let Some(mapped_ptr) = resolve_array_ptr(caller, mapped) {
                let mapped_len = read_array_length(caller, mapped_ptr).unwrap_or(0);
                for j in 0..mapped_len {
                    if let Some(inner) = read_array_elem(caller, mapped_ptr, j) {
                        elements.push(inner);
                    }
                }
            }
        } else {
            elements.push(mapped);
        }
    }
    let new_arr = alloc_array(caller, elements.len() as u32);
    let Some(new_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    for (i, elem) in elements.iter().enumerate() {
        write_array_elem(caller, new_ptr, i as u32, *elem);
    }
    write_array_length(caller, new_ptr, elements.len() as u32);
    new_arr
}

pub(crate) async fn proxy_trap_call_trap_with_args_async(
    caller: &mut Caller<'_, RuntimeState>,
    trap: i64,
    this_val: i64,
    args: &[i64],
) -> i64 {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .unwrap();
    let shadow_sp_global = caller
        .get_export("__shadow_sp")
        .and_then(|e| e.into_global())
        .unwrap();
    let saved_sp = shadow_sp_global.get(&mut *caller).i32().unwrap();
    let total_size = (args.len() * 8) as i32;
    let new_sp = saved_sp + total_size;
    for (i, &arg) in args.iter().enumerate() {
        memory
            .write(
                &mut *caller,
                (saved_sp + i as i32 * 8) as usize,
                &arg.to_le_bytes(),
            )
            .unwrap();
    }
    shadow_sp_global
        .set(&mut *caller, Val::I32(new_sp))
        .unwrap();
    let result = resolve_and_call_async(caller, trap, this_val, saved_sp, args.len() as i32).await;
    shadow_sp_global
        .set(&mut *caller, Val::I32(saved_sp))
        .unwrap();
    result
}

async fn proxy_trap_ordinary_get_by_name_id_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: i32,
) -> i64 {
    if value::is_proxy(target) {
        return Box::pin(proxy_trap_internal_get_async(caller, target, name_id)).await;
    }
    let Some(ptr) = resolve_handle(caller, target) else {
        return value::encode_undefined();
    };
    read_object_property_by_name_id(caller, ptr, name_id as u32)
        .unwrap_or_else(value::encode_undefined)
}

async fn proxy_trap_ordinary_set_by_name_id_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: i32,
    val: i64,
) -> bool {
    if value::is_proxy(target) {
        Box::pin(proxy_trap_internal_set_async(caller, target, name_id, val)).await;
        return true;
    }
    let Some(ptr) = resolve_handle(caller, target) else {
        return false;
    };
    write_object_property_by_name_id(
        caller,
        ptr,
        target,
        name_id as u32,
        val,
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE,
    );
    true
}

pub(crate) async fn proxy_trap_internal_get_async(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    name_id: i32,
) -> i64 {
    let (target, handler) = match proxy_trap_proxy_entry(caller, proxy, "get") {
        Ok(pair) => pair,
        Err(exc) => return exc,
    };
    if let Some(trap) = proxy_trap_handler_trap(caller, handler, "get") {
        let prop = proxy_trap_property_key_value(caller, name_id);
        return proxy_trap_call_trap_with_args_async(caller, trap, handler, &[target, prop, proxy])
            .await;
    }
    Box::pin(proxy_trap_ordinary_get_by_name_id_async(
        caller, target, name_id,
    ))
    .await
}

pub(crate) async fn proxy_trap_internal_set_async(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    name_id: i32,
    val: i64,
) {
    // 注意：set 内部方法返回 void（$obj_set 为 Type 9 `(i64,i32,i64)->()`），无法回传
    // TAG_EXCEPTION，故撤销代理上的 `proxy.x = v` 维持延迟（不可捕获）报错。规范要求的
    // 可捕获 [[Set]] 抛出经 Reflect.set（返回 i64，见 proxy_reflect_async）覆盖。
    let (target, handler) = match proxy_trap_proxy_entry(caller, proxy, "set") {
        Ok(pair) => pair,
        Err(_exc) => {
            set_runtime_error(
                caller.data(),
                "TypeError: Cannot perform 'set' on a proxy that has been revoked".to_string(),
            );
            return;
        }
    };
    if let Some(trap) = proxy_trap_handler_trap(caller, handler, "set") {
        let prop = proxy_trap_property_key_value(caller, name_id);
        let result = proxy_trap_call_trap_with_args_async(
            caller,
            trap,
            handler,
            &[target, prop, val, proxy],
        )
        .await;
        if !nanbox_to_bool(result) {
            set_runtime_error(
                caller.data(),
                "TypeError: Proxy set trap returned falsy".to_string(),
            );
        }
        return;
    }
    let _ = Box::pin(proxy_trap_ordinary_set_by_name_id_async(
        caller, target, name_id, val,
    ))
    .await;
}

pub(crate) async fn proxy_trap_internal_delete_async(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    name_id: i32,
) -> i64 {
    let (target, handler) = match proxy_trap_proxy_entry(caller, proxy, "deleteProperty") {
        Ok(pair) => pair,
        Err(exc) => return exc,
    };
    if let Some(trap) = proxy_trap_handler_trap(caller, handler, "deleteProperty") {
        let prop = proxy_trap_property_key_value(caller, name_id);
        let result =
            proxy_trap_call_trap_with_args_async(caller, trap, handler, &[target, prop]).await;
        return value::encode_bool(nanbox_to_bool(result));
    }
    value::encode_bool(true)
}

pub(crate) fn define_proxy_traps_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "proxy_trap_get",
        |mut caller: Caller<'_, RuntimeState>, (proxy, name_id): (i64, i32)| {
            Box::new(
                async move { proxy_trap_internal_get_async(&mut caller, proxy, name_id).await },
            )
        },
    )?;

    linker.func_wrap_async(
        "env",
        "proxy_trap_set",
        |mut caller: Caller<'_, RuntimeState>, (proxy, name_id, val): (i64, i32, i64)| {
            Box::new(async move {
                proxy_trap_internal_set_async(&mut caller, proxy, name_id, val).await;
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "proxy_trap_delete",
        |mut caller: Caller<'_, RuntimeState>, (proxy, name_id): (i64, i32)| {
            Box::new(
                async move { proxy_trap_internal_delete_async(&mut caller, proxy, name_id).await },
            )
        },
    )?;

    Ok(())
}

// ── TypedArray async callback overrides ──────────────────────────────────

use super::typedarray_new_methods::{sab_read, sab_write, ta_read, ta_resolve, ta_write};

async fn typedarray_sort_compare_async(
    caller: &mut Caller<'_, RuntimeState>,
    cmp: i64,
    a: i64,
    b: i64,
) -> std::cmp::Ordering {
    let result = call_wasm_callback_async(caller, cmp, value::encode_undefined(), &[a, b])
        .await
        .unwrap_or(value::encode_f64(0.0));
    let v = f64::from_bits(result as u64);
    if v > 0.0 {
        std::cmp::Ordering::Greater
    } else if v < 0.0 {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Equal
    }
}

async fn typedarray_proto_sort_async_body(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return this_val,
        };
    if length <= 1 {
        return this_val;
    }
    let mut elems: Vec<i64> = (0..length)
        .map(|i| {
            if is_shared {
                sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
            } else {
                ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
            }
            .unwrap_or(value::encode_undefined())
        })
        .collect();
    if args_count > 0 && value::is_callable(read_shadow_arg(caller, args_base, 0)) {
        let cmp = read_shadow_arg(caller, args_base, 0);
        for i in 0..elems.len() {
            for j in i + 1..elems.len() {
                if typedarray_sort_compare_async(caller, cmp, elems[i], elems[j]).await
                    == std::cmp::Ordering::Greater
                {
                    elems.swap(i, j);
                }
            }
        }
    } else {
        let keys: Vec<String> = elems
            .iter()
            .map(|e| render_value(caller, *e).unwrap_or_default())
            .collect();
        let mut indexed: Vec<(usize, &i64)> =
            (0..length as usize).map(|i| (i, &elems[i])).collect();
        indexed.sort_by(|(ia, _), (ib, _)| {
            let ka = &keys[*ia];
            let kb = &keys[*ib];
            let cmp = ka.cmp(kb);
            if cmp == std::cmp::Ordering::Equal {
                ia.cmp(ib)
            } else {
                cmp
            }
        });
        elems = indexed.iter().map(|(_, e)| **e).collect();
    }
    for (i, &elem) in elems.iter().enumerate() {
        if is_shared {
            sab_write(
                caller,
                buf_handle,
                byte_offset,
                elem_size,
                element_kind,
                i as u32,
                elem,
            );
        } else {
            ta_write(
                caller,
                buf_handle,
                byte_offset,
                elem_size,
                element_kind,
                i as u32,
                elem,
            );
        };
    }
    this_val
}

async fn typedarray_proto_for_each_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_undefined(),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val])
            .await
            .is_err()
        {
            return value::encode_undefined();
        }
    }
    value::encode_undefined()
}

async fn typedarray_proto_map_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_undefined(),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    let new_arr = alloc_array(caller, length);
    let Some(arr_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        let mapped = match call_wasm_callback_async(
            caller,
            cb,
            this_arg,
            &[elem, idx_val, this_val],
        )
        .await
        {
            Ok(v) => v,
            Err(_) => return value::encode_undefined(),
        };
        write_array_elem(caller, arr_ptr, i, mapped);
    }
    write_array_length(caller, arr_ptr, length);
    new_arr
}

async fn typedarray_proto_filter_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_undefined(),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    let mut results = Vec::new();
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        let keep = match call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val])
            .await
        {
            Ok(v) => value::is_truthy(v),
            Err(_) => return value::encode_undefined(),
        };
        if keep {
            results.push(elem);
        }
    }
    let new_arr = alloc_array(caller, results.len() as u32);
    let Some(arr_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    for (j, elem) in results.iter().enumerate() {
        write_array_elem(caller, arr_ptr, j as u32, *elem);
    }
    write_array_length(caller, arr_ptr, results.len() as u32);
    new_arr
}

async fn typedarray_proto_reduce_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_undefined(),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let has_init = args_count > 1;
    let init = if has_init {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    if length == 0 && !has_init {
        return value::encode_undefined();
    }
    let mut acc = if has_init {
        init
    } else {
        if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, 0)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, 0)
        }
        .unwrap_or(value::encode_undefined())
    };
    let start = if has_init { 0 } else { 1 };
    for i in start..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        acc = match call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[acc, elem, idx_val, this_val],
        )
        .await
        {
            Ok(v) => v,
            Err(_) => return value::encode_undefined(),
        };
    }
    acc
}

async fn typedarray_proto_reduce_right_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_undefined(),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let has_init = args_count > 1;
    let init = if has_init {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    if length == 0 && !has_init {
        return value::encode_undefined();
    }
    let mut acc = if has_init {
        init
    } else {
        if is_shared {
            sab_read(
                caller,
                buf_handle,
                byte_offset,
                elem_size,
                element_kind,
                length - 1,
            )
        } else {
            ta_read(
                caller,
                buf_handle,
                byte_offset,
                elem_size,
                element_kind,
                length - 1,
            )
        }
        .unwrap_or(value::encode_undefined())
    };
    let end = if has_init {
        length as i32 - 1
    } else {
        length as i32 - 2
    };
    for i in (0..=end as u32).rev() {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        acc = match call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[acc, elem, idx_val, this_val],
        )
        .await
        {
            Ok(v) => v,
            Err(_) => return value::encode_undefined(),
        };
    }
    acc
}

async fn typedarray_proto_find_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_undefined(),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if _args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) =
            call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val]).await
            && value::is_truthy(r)
        {
            return elem;
        }
    }
    value::encode_undefined()
}

async fn typedarray_proto_find_index_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_f64(-1.0),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_f64(-1.0);
    }
    let this_arg = if _args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) =
            call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val]).await
            && value::is_truthy(r)
        {
            return value::encode_f64(i as f64);
        }
    }
    value::encode_f64(-1.0)
}

async fn typedarray_proto_some_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_bool(false),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_bool(false);
    }
    let this_arg = if _args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) =
            call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val]).await
            && value::is_truthy(r)
        {
            return value::encode_bool(true);
        }
    }
    value::encode_bool(false)
}

async fn typedarray_proto_every_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_bool(true),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_bool(true);
    }
    let this_arg = if _args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        match call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val]).await {
            Ok(r) => {
                if !value::is_truthy(r) {
                    return value::encode_bool(false);
                }
            }
            Err(_) => return value::encode_bool(false),
        }
    }
    value::encode_bool(true)
}

macro_rules! wrap_typedarray_callback_async {
    ($linker:expr, $name:expr, $body:expr) => {
        $linker.func_wrap_async(
            "env",
            $name,
            |mut caller: Caller<'_, RuntimeState>,
             (_env_obj, this_val, args_base, args_count): (i64, i64, i32, i32)| {
                Box::new(async move { $body(&mut caller, this_val, args_base, args_count).await })
            },
        )?;
    };
}

pub(crate) fn define_typedarray_new_methods_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "typedarray_proto_sort",
        |mut caller: Caller<'_, RuntimeState>,
         (_env_obj, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                typedarray_proto_sort_async_body(&mut caller, this_val, args_base, args_count).await
            })
        },
    )?;

    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_for_each",
        typedarray_proto_for_each_async
    );
    wrap_typedarray_callback_async!(linker, "typedarray_proto_map", typedarray_proto_map_async);
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_filter",
        typedarray_proto_filter_async
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_reduce",
        typedarray_proto_reduce_async
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_reduce_right",
        typedarray_proto_reduce_right_async
    );
    wrap_typedarray_callback_async!(linker, "typedarray_proto_find", typedarray_proto_find_async);
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_find_index",
        typedarray_proto_find_index_async
    );
    wrap_typedarray_callback_async!(linker, "typedarray_proto_some", typedarray_proto_some_async);
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_every",
        typedarray_proto_every_async
    );

    Ok(())
}

// ── Primitive-core async callback override (string_replace) ────────────────────

/// 从预收集的命名组数据构建捕获组对象（Send-safe — 不持有 regress::Match 引用）
fn build_groups_obj_from_named(
    caller: &mut Caller<'_, RuntimeState>,
    named: &[(String, Option<std::ops::Range<usize>>)],
    s: &str,
) -> i64 {
    if named.is_empty() {
        return value::encode_undefined();
    }
    let obj = {
        let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_null_proto_object(caller, &_wjsm_env, named.len() as u32)
    };
    for (name, range) in named {
        let val = match range {
            Some(r) => store_runtime_string(caller, s[r.clone()].to_string()),
            None => value::encode_undefined(),
        };
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
    }
    obj
}

/// 处理 JavaScript 替换模式（Send-safe — 不持有 regress::Match 引用）
fn process_replacement_from_captures(
    replace_str: &str,
    s: &str,
    match_start: usize,
    match_end: usize,
    captures: &[Option<std::ops::Range<usize>>],
    named: &[(String, Option<std::ops::Range<usize>>)],
) -> String {
    let mut result = String::new();
    let chars: Vec<char> = replace_str.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            let next = chars[i + 1];
            match next {
                '$' => {
                    result.push('$');
                    i += 2;
                }
                '&' => {
                    result.push_str(&s[match_start..match_end]);
                    i += 2;
                }
                '`' => {
                    result.push_str(&s[..match_start]);
                    i += 2;
                }
                '\'' => {
                    result.push_str(&s[match_end..]);
                    i += 2;
                }
                '<' => {
                    // $<name> → named capture group
                    if let Some(close_pos) = chars[i + 2..].iter().position(|&c| c == '>') {
                        let name: String = chars[i + 2..i + 2 + close_pos].iter().collect();
                        if let Some((_, range)) = named.iter().find(|(n, _)| n == &name) {
                            if let Some(r) = range {
                                result.push_str(&s[r.clone()]);
                            }
                        }
                        // 命名组不存在或未匹配 → 空字符串（ES 规范）
                        i += 3 + close_pos; // skip past $<name>
                    } else {
                        // 未闭合的 $<，保持原样
                        result.push('$');
                        result.push('<');
                        i += 2;
                    }
                }
                '0'..='9' => {
                    // $n or $nn → captured group
                    let mut group_num = (next as u8 - b'0') as usize;
                    let mut consumed = 2;
                    // ECMAScript: $0 不是特殊模式，应保持字面量
                    if group_num == 0 {
                        result.push('$');
                        result.push('0');
                        i += 2;
                        continue;
                    }
                    // 检查是否为两位数 $nn
                    if i + 2 < chars.len()
                        && let Some('0'..='9') = chars.get(i + 2)
                    {
                        let next_digit = (chars[i + 2] as u8 - b'0') as usize;
                        let two_digit = group_num * 10 + next_digit;
                        // $00 不是特殊模式，只有 $01-$99 是
                        if two_digit > 0 && two_digit <= captures.len() {
                            group_num = two_digit;
                            consumed = 3;
                        }
                    }
                    // 获取捕获组（group_num ≥ 1）
                    if group_num <= captures.len() {
                        if let Some(Some(range)) = captures.get(group_num) {
                            result.push_str(&s[range.clone()]);
                        }
                    } else {
                        result.push('$');
                        result.push(next);
                    }
                    i += consumed;
                }
                _ => {
                    result.push('$');
                    result.push(next);
                    i += 2;
                }
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

fn replace_callback_result_to_string(caller: &mut Caller<'_, RuntimeState>, result: i64) -> String {
    if value::is_undefined(result) {
        return String::new();
    }
    if value::is_runtime_string_handle(result) || value::is_string(result) {
        return get_string_value(caller, result);
    }
    eval_to_string(caller, result)
}

/// Async version of call_replace_func — uses shared Wasm callback shadow-stack handling.
async fn call_replace_func_async(
    caller: &mut Caller<'_, RuntimeState>,
    func: i64,
    s: &str,
    match_start: usize,
    match_end: usize,
    captures: &[Option<std::ops::Range<usize>>],
    named_groups_obj: i64,
) -> String {
    let capture_count = captures.len().saturating_sub(1);
    let mut args = Vec::with_capacity(1 + capture_count + 3);

    args.push(store_runtime_string(
        &*caller,
        s[match_start..match_end].to_string(),
    ));
    for i in 1..=capture_count {
        let capture_val = if let Some(Some(range)) = captures.get(i) {
            store_runtime_string(&*caller, s[range.clone()].to_string())
        } else {
            value::encode_undefined()
        };
        args.push(capture_val);
    }
    args.push(value::encode_f64(match_start as f64));
    args.push(store_runtime_string(&*caller, s.to_string()));
    args.push(named_groups_obj);

    let result = call_wasm_callback_async(caller, func, value::encode_undefined(), &args)
        .await
        .unwrap_or_else(|_| value::encode_undefined());
    replace_callback_result_to_string(caller, result)
}

async fn string_replace_async_body(
    mut caller: Caller<'_, RuntimeState>,
    receiver: i64,
    search: i64,
    replace: i64,
) -> i64 {
    let s = get_string_value(&mut caller, receiver);

    // 检查 replace 是否为函数（支持函数替换）
    let is_func_replace = value::is_callable(replace);

    if value::is_regexp(search) {
        let entry = {
            let table = caller.data().regex_table.lock().unwrap();
            match table.get(value::decode_regexp_handle(search) as usize) {
                Some(e) => e.clone(),
                None => return store_runtime_string(&caller, s),
            }
        };

        let is_global = entry.flags.contains('g');
        if is_global {
            // 全局替换：先收集所有匹配数据（避免 find_iter / Match 跨 await 导致非 Send）
            struct MatchInfo {
                start: usize,
                end: usize,
                captures: Vec<Option<std::ops::Range<usize>>>,
                named: Vec<(String, Option<std::ops::Range<usize>>)>,
            }
            let matches: Vec<MatchInfo> = entry
                .compiled
                .find_iter(&s)
                .map(|m| MatchInfo {
                    start: m.start(),
                    end: m.end(),
                    captures: (0..m.captures.len() + 1).map(|i| m.group(i)).collect(),
                    named: m
                        .named_groups()
                        .map(|(name, range)| (name.to_string(), range))
                        .collect(),
                })
                .collect();

            let mut result = String::new();
            let mut last_end = 0;
            for mi in &matches {
                // 添加匹配前的部分
                result.push_str(&s[last_end..mi.start]);
                // 根据是否为函数选择替换方式
                let replaced = if is_func_replace {
                    let groups_obj = if mi.named.is_empty() {
                        value::encode_undefined()
                    } else {
                        build_groups_obj_from_named(&mut caller, &mi.named, &s)
                    };
                    call_replace_func_async(
                        &mut caller,
                        replace,
                        &s,
                        mi.start,
                        mi.end,
                        &mi.captures,
                        groups_obj,
                    )
                    .await
                } else {
                    let replace_str = get_string_value(&mut caller, replace);
                    process_replacement_from_captures(
                        &replace_str,
                        &s,
                        mi.start,
                        mi.end,
                        &mi.captures,
                        &mi.named,
                    )
                };
                result.push_str(&replaced);
                last_end = mi.end;
            }
            result.push_str(&s[last_end..]);
            store_runtime_string(&caller, result)
        } else {
            // 单次替换
            match entry.compiled.find(&s) {
                Some(m) => {
                    let captures: Vec<Option<std::ops::Range<usize>>> =
                        (0..m.captures.len() + 1).map(|i| m.group(i)).collect();
                    let match_start = m.start();
                    let match_end = m.end();
                    let named: Vec<(String, Option<std::ops::Range<usize>>)> = m
                        .named_groups()
                        .map(|(name, range)| (name.to_string(), range))
                        .collect();
                    let groups_obj = if named.is_empty() {
                        value::encode_undefined()
                    } else {
                        build_groups_obj_from_named(&mut caller, &named, &s)
                    };
                    // Match 不再使用 — 所有数据已提取
                    let replaced = if is_func_replace {
                        call_replace_func_async(
                            &mut caller,
                            replace,
                            &s,
                            match_start,
                            match_end,
                            &captures,
                            groups_obj,
                        )
                        .await
                    } else {
                        let replace_str = get_string_value(&mut caller, replace);
                        process_replacement_from_captures(
                            &replace_str,
                            &s,
                            match_start,
                            match_end,
                            &captures,
                            &named,
                        )
                    };
                    let mut result = String::new();
                    result.push_str(&s[..match_start]);
                    result.push_str(&replaced);
                    result.push_str(&s[match_end..]);
                    store_runtime_string(&caller, result)
                }
                None => store_runtime_string(&caller, s),
            }
        }
    } else {
        // 字符串替换
        let search_str = get_string_value(&mut caller, search);
        if let Some(pos) = s.find(&search_str) {
            // 对于字符串搜索，函数替换的参数是：matched, offset, string
            let replaced = if is_func_replace {
                // 构造 captures（只有完整匹配）
                let captures = vec![Some(pos..pos + search_str.len())];
                call_replace_func_async(
                    &mut caller,
                    replace,
                    &s,
                    pos,
                    pos + search_str.len(),
                    &captures,
                    value::encode_undefined(),
                )
                .await
            } else {
                get_string_value(&mut caller, replace)
            };
            let mut result = String::new();
            result.push_str(&s[..pos]);
            result.push_str(&replaced);
            result.push_str(&s[pos + search_str.len()..]);
            store_runtime_string(&caller, result)
        } else {
            store_runtime_string(&caller, s)
        }
    }
}

pub(crate) fn define_primitive_core_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "string_replace",
        |mut caller: Caller<'_, RuntimeState>, (receiver, search, replace): (i64, i64, i64)| {
            Box::new(
                async move { string_replace_async_body(caller, receiver, search, replace).await },
            )
        },
    )?;
    Ok(())
}
