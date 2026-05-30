use super::*;

pub(crate) fn clear_pending_unhandled_rejection(state: &RuntimeState, handle: usize) {
    state
        .pending_unhandled_rejections
        .lock()
        .expect("pending_unhandled_rejections mutex")
        .remove(&handle);
}

pub(crate) fn drain_microtasks<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
) {
    loop {
        let task = {
            let mut queue = ctx
                .state_mut()
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
                if handle_combinator_reaction(ctx, env, handler, argument) {
                    continue;
                }
                if value::is_callable(handler) {
                    let call_arg = match reaction_type {
                        ReactionType::FinallyFulfill | ReactionType::FinallyReject => {
                            value::encode_undefined()
                        }
                        _ => argument,
                    };
                    match call_host_function(ctx, env, handler, call_arg) {
                        Some(result) => match reaction_type {
                            ReactionType::Fulfill | ReactionType::Reject => {
                                resolve_promise(ctx, env, promise, result);
                            }
                            ReactionType::FinallyFulfill => {
                                settle_promise(
                                    ctx.state_mut(),
                                    promise,
                                    PromiseSettlement::Fulfill(argument),
                                );
                            }
                            ReactionType::FinallyReject => {
                                settle_promise(
                                    ctx.state_mut(),
                                    promise,
                                    PromiseSettlement::Reject(argument),
                                );
                            }
                        },
                        None => {
                            let err = runtime_error_value(
                                ctx.state_mut(),
                                "TypeError: promise reaction handler failed".to_string(),
                            );
                            settle_promise(
                                ctx.state_mut(),
                                promise,
                                PromiseSettlement::Reject(err),
                            );
                        }
                    }
                } else {
                    let settlement = passive_reaction_settlement(reaction_type, argument);
                    settle_promise(ctx.state_mut(), promise, settlement);
                }
            }
            Some(Microtask::PromiseResolveThenable {
                promise,
                thenable,
                then,
            }) => {
                let (resolve, reject) =
                    create_promise_resolving_functions(ctx.state_mut(), promise);
                if call_host_function(ctx, env, then, resolve).is_none() {
                    settle_promise(ctx.state_mut(), promise, PromiseSettlement::Reject(reject));
                }
                let _ = thenable;
            }
            Some(Microtask::MicrotaskCallback { callback }) => {
                if value::is_callable(callback) {
                    let _ = call_host_function_with_args(
                        ctx,
                        env,
                        callback,
                        value::encode_undefined(),
                        &[],
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
                resume_async_function(
                    ctx,
                    env,
                    fn_table_idx,
                    continuation,
                    state,
                    resume_val,
                    is_rejected,
                );
            }
            Some(Microtask::CleanupFinalizationRegistry {
                callback,
                held_value,
            }) => {
                ctx.state_mut()
                    .pending_cleanup_callbacks
                    .lock()
                    .expect("pending_cleanup_callbacks mutex")
                    .push((callback, vec![held_value]));
            }
            None => break,
        }
    }
    {
        let mut c_table = ctx
            .state_mut()
            .continuation_table
            .lock()
            .expect("continuation table mutex");
        c_table.retain(|entry| !entry.completed);
    }
    let unhandled: Vec<i64> = {
        let rejections = std::mem::take(
            &mut *ctx
                .state_mut()
                .pending_unhandled_rejections
                .lock()
                .expect("pending_unhandled_rejections mutex"),
        );
        let table = ctx
            .state_mut()
            .promise_table
            .lock()
            .expect("promise table mutex");
        rejections
            .iter()
            .filter_map(|&h| {
                let entry = table.get(h).filter(|e| e.is_promise)?;
                if entry.handled {
                    return None;
                }
                match entry.state {
                    PromiseState::Rejected(reason) => Some(reason),
                    _ => None,
                }
            })
            .collect()
    };
    for reason in unhandled {
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

#[inline]
pub(crate) fn drain_microtasks_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    _func_table: &Table,
) {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    drain_microtasks(caller, &env);
}

pub(crate) fn call_host_function<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    handler: i64,
    argument: i64,
) -> Option<i64> {
    call_host_function_with_args(ctx, env, handler, value::encode_undefined(), &[argument])
}

pub(crate) fn call_host_function_with_args<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
    handler: i64,
    this_val: i64,
    args: &[i64],
) -> Option<i64> {
    if value::is_bound(handler) {
        let bound_idx = value::decode_bound_idx(handler);
        let (target_func, bound_this, mut combined_args) = {
            let state = ctx.state_mut();
            let bound = state.bound_objects.lock().unwrap();
            let record = &bound[bound_idx as usize];
            (
                record.target_func,
                record.bound_this,
                record.bound_args.clone(),
            )
        };
        combined_args.extend_from_slice(args);
        return call_host_function_with_args(ctx, env, target_func, bound_this, &combined_args);
    }

    let (func_idx, env_obj) = {
        let state = ctx.state_mut();
        if value::is_closure(handler) {
            let idx = value::decode_closure_idx(handler);
            let closures = state.closures.lock().unwrap();
            let entry = &closures[idx as usize];
            (entry.func_idx, entry.env_obj)
        } else if value::is_function(handler) {
            (
                value::decode_function_idx(handler),
                value::encode_undefined(),
            )
        } else {
            return None;
        }
    };

    let saved_sp = env.shadow_sp.get(&mut *ctx).i32().unwrap_or(0);
    let args_bytes = args.len().checked_mul(8)?;
    {
        let data = env.memory.data_mut(&mut *ctx);
        let offset = saved_sp as usize;
        if offset + args_bytes > data.len() {
            return None;
        }
        for (index, arg) in args.iter().enumerate() {
            let write_offset = offset + index * 8;
            data[write_offset..write_offset + 8].copy_from_slice(&arg.to_le_bytes());
        }
    }
    let new_sp = saved_sp + (args.len() as i32) * 8;
    let _ = env.shadow_sp.set(&mut *ctx, Val::I32(new_sp));

    let func_ref = env.func_table.get(&mut *ctx, func_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else {
        let _ = env.shadow_sp.set(&mut *ctx, Val::I32(saved_sp));
        return None;
    };
    let previous_new_target = ctx
        .state_mut()
        .new_target
        .replace(value::encode_undefined());
    let mut results = [Val::I64(0)];
    let call_result = func.call(
        &mut *ctx,
        &[
            Val::I64(env_obj),
            Val::I64(this_val),
            Val::I32(saved_sp),
            Val::I32(args.len() as i32),
        ],
        &mut results,
    );
    ctx.state_mut().new_target.set(previous_new_target);
    let _ = env.shadow_sp.set(&mut *ctx, Val::I32(saved_sp));

    if let Err(err) = call_result {
        set_runtime_error(
            ctx.state_mut(),
            format!("host function callback error: {err}"),
        );
        return None;
    }

    results[0].i64()
}

#[inline]
pub(crate) fn call_host_function_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    _func_table: &Table,
    handler: i64,
    argument: i64,
) -> Option<i64> {
    if value::is_native_callable(handler) {
        return call_native_callable_from_caller(caller, handler, Some(argument));
    }
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    call_host_function(caller, &env, handler, argument)
}
