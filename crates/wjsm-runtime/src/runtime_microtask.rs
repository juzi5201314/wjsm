use super::*;
use std::sync::atomic::Ordering;
pub(crate) fn clear_pending_unhandled_rejection(state: &RuntimeState, handle: usize) {
    state
        .pending_unhandled_rejections
        .lock()
        .expect("pending_unhandled_rejections mutex")
        .remove(&handle);
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
        .swap(value::encode_undefined(), Ordering::Relaxed);
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
    ctx.state_mut()
        .new_target
        .store(previous_new_target, Ordering::Relaxed);
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

/// Phase 3 must-convert 之 drain_microtasks + call_host_function 路径（按 2026-05-31-async-scheduler-implementation-plan.md + 原始 Phase 3 audit must-convert list 顶部遗留条目 + docs/superpowers/specs/2026-05-26-async-audit-refactor-design.md + 26-async-audit-refactor.md）：
///
/// 为 runtime_microtask.rs 中的 `drain_microtasks`（含完整 microtask 队列 loop + Microtask 七个变体的固定 pop_front 顺序 + bound 递归 + unhandled rejection 报告 + continuation retain）及 `call_host_function*` 添加 async 孪生版本，与 sync 版本并存。
///
/// 规则（严格 1:1 遵循先前所有成功 Phase 3 转换如 resume_async_function_async / call_wasm_callback_async / try_compiled_eval... 的 narrow boring 模式）：
/// - 所有现有 sync 函数体、调用点、thin from_caller wrapper 行完全不动（existing callers 继续用 sync）
/// - async 版本控制流、match 臂顺序、错误路径、settle/resolve 副作用、递归 bound 展开 100% 相同
/// - 唯一差异：
///     1. resume_async_function 调用点 → resume_async_function_async(...).await
///     2. call_host_function / call_host_function_with_args 调用点 → _async(...).await
///     3. call_host_with_args 内部的 func.call(...) → func.call_async(...).await
/// - 宿主重入：当前 async 版本仍走 call_host 路径（为未来接 call_wasm_callback_async 预留）；bound 递归使用 _async 自身 .await 保证正确
/// - drain_async 因 resume_async_function_async 采用 Store 具体类型（先前转换选择），此处使用 Store 以匹配调用并保持 body 内 ctx. 文本等价；call_host_*_async 保持泛型支持 Caller/Store
/// - 中文 header 引用 2026-05-31 plan、async Store contract（Correction 3：yield 后所有 Wasm entry 必须 async API）、原始 must-convert 列表
///
/// 完成此项后，Phase 3 must-convert audit 列表（eval、resume、host reentrant、microtask+call_host）全部落地，'Phase 1-4 solid' 可被诚实评估。
pub(crate) async fn drain_microtasks_async<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
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
                    match call_host_function_async(ctx, env, handler, call_arg).await {
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
                if call_host_function_async(ctx, env, then, resolve)
                    .await
                    .is_none()
                {
                    settle_promise(ctx.state_mut(), promise, PromiseSettlement::Reject(reject));
                }
                let _ = thenable;
            }
            Some(Microtask::MicrotaskCallback { callback }) => {
                if value::is_callable(callback) {
                    let _ = call_host_function_with_args_async(
                        ctx,
                        env,
                        callback,
                        value::encode_undefined(),
                        &[],
                    )
                    .await;
                }
            }
            Some(Microtask::TransformStreamTransform {
                callback,
                this_val,
                chunk,
                controller,
                write_promise,
            }) => {
                let result = call_host_function_with_args_async(
                    ctx,
                    env,
                    callback,
                    this_val,
                    &[chunk, controller],
                )
                .await;
                match result {
                    Some(result) if is_promise_value(ctx.state_mut(), result) => {
                        resolve_promise(ctx, env, write_promise, result);
                    }
                    Some(_) => {
                        settle_promise(
                            ctx.state_mut(),
                            write_promise,
                            PromiseSettlement::Fulfill(value::encode_undefined()),
                        );
                    }
                    None => {
                        let err = runtime_error_value(
                            ctx.state_mut(),
                            "TypeError: TransformStream transform callback failed".to_string(),
                        );
                        settle_promise(
                            ctx.state_mut(),
                            write_promise,
                            PromiseSettlement::Reject(err),
                        );
                    }
                }
            }
            Some(Microtask::TransformStreamFlush {
                callback,
                this_val,
                controller,
                readable_stream_handle,
                readable_controller_handle,
                close_promise,
            }) => {
                let flush_ok = match callback {
                    Some(callback) => call_host_function_with_args_async(
                        ctx,
                        env,
                        callback,
                        this_val,
                        &[controller],
                    )
                    .await
                    .is_some(),
                    None => true,
                };
                if !flush_ok {
                    let err = runtime_error_value(
                        ctx.state_mut(),
                        "TypeError: TransformStream flush callback failed".to_string(),
                    );
                    settle_promise(
                        ctx.state_mut(),
                        close_promise,
                        PromiseSettlement::Reject(err),
                    );
                    continue;
                }

                {
                    let mut ctrl_table = ctx
                        .state_mut()
                        .stream_controller_table
                        .lock()
                        .expect("controller mutex");
                    if let Some(ctrl) = ctrl_table.get_mut(readable_controller_handle as usize) {
                        ctrl.close_requested = true;
                    }
                }

                let pending = {
                    let mut reader_table =
                        ctx.state_mut().reader_table.lock().expect("reader mutex");
                    let mut pending_promise: Option<i64> = None;
                    for reader in reader_table.iter_mut() {
                        if reader.stream_handle == readable_stream_handle {
                            if let Some(promise) = reader.pending_read_promise.take() {
                                pending_promise = Some(promise);
                                break;
                            }
                        }
                    }
                    pending_promise
                };

                {
                    let mut stream_table = ctx
                        .state_mut()
                        .readable_stream_table
                        .lock()
                        .expect("stream mutex");
                    if let Some(entry) = stream_table.get_mut(readable_stream_handle as usize) {
                        entry.state = StreamState::Closed;
                    }
                }

                if let Some(promise) = pending {
                    let result = build_reader_result_with_env(ctx, env, true, None);
                    settle_promise(ctx.state_mut(), promise, PromiseSettlement::Fulfill(result));
                }
                settle_promise(
                    ctx.state_mut(),
                    close_promise,
                    PromiseSettlement::Fulfill(value::encode_undefined()),
                );
            }
            Some(Microtask::AsyncResume {
                fn_table_idx,
                continuation,
                state,
                resume_val,
                is_rejected,
            }) => {
                resume_async_function_async(
                    ctx,
                    env,
                    fn_table_idx,
                    continuation,
                    state,
                    resume_val,
                    is_rejected,
                )
                .await;
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
            Some(Microtask::ReadableStreamPull {
                callback,
                this_val,
                controller,
            }) => {
                let _ =
                    call_host_function_with_args_async(ctx, env, callback, this_val, &[controller])
                        .await;
            }
            None => break,
        }
    }
    crate::runtime_async_fn::recycle_completed_continuations(ctx.state_mut());
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
pub(crate) async fn drain_microtasks_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    _func_table: &Table,
) {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    drain_microtasks_async(caller, &env).await;
}

pub(crate) async fn call_host_function_async<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
    handler: i64,
    argument: i64,
) -> Option<i64> {
    call_host_function_with_args_async(ctx, env, handler, value::encode_undefined(), &[argument])
        .await
}

pub(crate) async fn call_host_function_with_args_async<
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
        return Box::pin(call_host_function_with_args_async(
            ctx,
            env,
            target_func,
            bound_this,
            &combined_args,
        ))
        .await;
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
        .swap(value::encode_undefined(), Ordering::Relaxed);
    let mut results = [Val::I64(0)];
    // 唯一差异：与本 crate 其他 Phase 3 async 转换（resume、call_wasm_callback）一致
    let call_result = func
        .call_async(
            &mut *ctx,
            &[
                Val::I64(env_obj),
                Val::I64(this_val),
                Val::I32(saved_sp),
                Val::I32(args.len() as i32),
            ],
            &mut results,
        )
        .await;
    ctx.state_mut()
        .new_target
        .store(previous_new_target, Ordering::Relaxed);
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
pub(crate) async fn call_host_function_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    _func_table: &Table,
    handler: i64,
    argument: i64,
) -> Option<i64> {
    if value::is_native_callable(handler) {
        return call_native_callable_with_args_from_caller_async(
            caller,
            handler,
            value::encode_undefined(),
            vec![argument],
        )
        .await;
    }
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    call_host_function_async(caller, &env, handler, argument).await
}
