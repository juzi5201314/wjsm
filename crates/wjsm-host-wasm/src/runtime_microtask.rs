use super::*;
pub(crate) fn clear_pending_unhandled_rejection(state: &RuntimeState, handle: usize) {
    state
        .pending_unhandled_rejections
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|pending| *pending != handle);
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
fn with_captured_scope_enter(
    ctx: &mut impl crate::RuntimeStateAccess,
    scope: Option<crate::CapturedScope>,
) -> Option<(crate::CapturedScope, Option<crate::FrameId>)> {
    let scope = scope?;
    let mut hooks = ctx
        .state_mut()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let prior = hooks.enter_captured_scope(scope);
    Some((scope, prior))
}

fn with_captured_scope_exit(
    ctx: &mut impl crate::RuntimeStateAccess,
    entered: Option<(crate::CapturedScope, Option<crate::FrameId>)>,
) {
    if let Some((scope, prior)) = entered {
        let mut hooks = ctx
            .state_mut()
            .async_hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        hooks.exit_captured_scope(scope, prior);
    }
}

pub(crate) async fn drain_microtasks_async<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
) {
    loop {
        if crate::runtime_async_hooks::emit::drain_pending_promise_events(ctx, env).await {
            return;
        }
        if crate::runtime_async_hooks::emit::drain_destroy_queue(ctx, env).await {
            return;
        }
        loop {
            let next_tick = {
                let mut queue = ctx
                    .state_mut()
                    .next_tick_queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                queue.pop_front()
            };
            let Some(next_tick) = next_tick else {
                break;
            };
            let prior = if let Some(scope) = next_tick.scope {
                let mut hooks = ctx
                    .state_mut()
                    .async_hooks
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                Some((scope, hooks.enter_captured_scope(scope)))
            } else {
                None
            };
            if let Some(scope) = next_tick.scope
                && crate::runtime_async_hooks::emit::emit_before(ctx, env, scope.async_id, false)
                    .await
            {
                return;
            }

            if is_callable_with_env(ctx, env, next_tick.callback) {
                let _ = call_host_function_with_args_async(
                    ctx,
                    env,
                    next_tick.callback,
                    value::encode_undefined(),
                    &next_tick.args,
                )
                .await;
            }
            if let Some(scope) = next_tick.scope
                && crate::runtime_async_hooks::emit::emit_after(ctx, env, scope.async_id, false)
                    .await
            {
                return;
            }

            if let Some((scope, prior_frame)) = prior {
                let mut hooks = ctx
                    .state_mut()
                    .async_hooks
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                hooks.exit_captured_scope(scope, prior_frame);
            }
            if let Some(scope) = next_tick.scope
                && crate::runtime_async_hooks::emit::emit_destroy(ctx, env, scope.async_id, false)
                    .await
            {
                return;
            }

            if crate::runtime_process::pending_process_exit_signal(ctx.state_mut()).is_some() {
                return;
            }
        }

        // setImmediate：nextTick 之后、普通 microtask/timers 之前
        crate::runtime_node_async_hooks::drain_immediates_async(ctx, env).await;
        if crate::runtime_process::pending_process_exit_signal(ctx.state_mut()).is_some() {
            return;
        }

        let task = {
            let mut queue = ctx
                .state_mut()
                .microtask_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            queue.pop_front()
        };
        match task {
            Some(Microtask::PromiseReaction {
                promise,
                reaction_type,
                handler,
                argument,
                scope,
            }) => {
                let entered = with_captured_scope_enter(ctx, scope);
                if let Some(scope) = scope
                    && crate::runtime_async_hooks::emit::emit_before(ctx, env, scope.async_id, true)
                        .await
                {
                    return;
                }

                let handled_internally = handle_combinator_reaction(ctx, env, handler, argument)
                    || handle_finally_await_reaction(ctx, handler, argument, reaction_type);
                if !handled_internally {
                    if is_callable_with_env(ctx, env, handler) {
                        let call_arg = match reaction_type {
                            ReactionType::FinallyFulfill | ReactionType::FinallyReject => {
                                value::encode_undefined()
                            }
                            _ => argument,
                        };
                        match call_host_function_async(ctx, env, handler, call_arg).await {
                            Some(result) => match reaction_type {
                                ReactionType::Fulfill | ReactionType::Reject => {
                                    // onFulfilled/onRejected 抛异常时（§27.2.1.3.2 step），
                                    // result promise 应以抛出的值 reject，而非 fulfill 异常值。
                                    if value::is_exception(result) {
                                        let reason =
                                            exception_reason_from_state(ctx.state_mut(), result);
                                        settle_promise(
                                            ctx.state_mut(),
                                            promise,
                                            PromiseSettlement::Reject(reason),
                                        );
                                    } else {
                                        resolve_promise(ctx, env, promise, result);
                                    }
                                }
                                ReactionType::FinallyFulfill | ReactionType::FinallyReject => {
                                    settle_finally_reaction(
                                        ctx,
                                        env,
                                        promise,
                                        argument,
                                        result,
                                        reaction_type,
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
                if crate::runtime_async_hooks::emit::drain_pending_promise_events(ctx, env).await {
                    return;
                }
                if let Some(scope) = scope
                    && crate::runtime_async_hooks::emit::emit_after(ctx, env, scope.async_id, true)
                        .await
                {
                    return;
                }
                with_captured_scope_exit(ctx, entered);
            }
            Some(Microtask::PromiseResolveThenable {
                promise,
                thenable,
                then,
            }) => {
                let (resolve, reject) =
                    create_promise_resolving_functions(ctx.state_mut(), promise);
                let result = call_host_function_with_args_async(
                    ctx,
                    env,
                    then,
                    thenable,
                    &[resolve, reject],
                )
                .await;
                match result {
                    Some(result) if value::is_exception(result) => {
                        let reason = exception_reason_from_state(ctx.state_mut(), result);
                        settle_promise(ctx.state_mut(), promise, PromiseSettlement::Reject(reason));
                    }
                    Some(_) => {}
                    None => {
                        let err = runtime_error_value(
                            ctx.state_mut(),
                            "TypeError: PromiseResolveThenable then failed".to_string(),
                        );
                        settle_promise(ctx.state_mut(), promise, PromiseSettlement::Reject(err));
                    }
                }
            }
            Some(Microtask::MicrotaskCallback { callback, scope }) => {
                let entered = with_captured_scope_enter(ctx, scope);
                if is_callable_with_env(ctx, env, callback) {
                    let _ = call_host_function_with_args_async(
                        ctx,
                        env,
                        callback,
                        value::encode_undefined(),
                        &[],
                    )
                    .await;
                }
                with_captured_scope_exit(ctx, entered);
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
                writable_stream_handle,
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
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if let Some(ctrl) = ctrl_table.get_mut(readable_controller_handle as usize) {
                        ctrl.close_requested = true;
                    }
                }

                let pending = {
                    let mut reader_table = ctx
                        .state_mut()
                        .reader_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let mut pending_promise: Option<i64> = None;
                    for reader in reader_table.iter_mut() {
                        if reader.stream_handle == readable_stream_handle
                            && let Some(promise) = reader.pending_read_promise.take()
                        {
                            pending_promise = Some(promise);
                            break;
                        }
                    }
                    pending_promise
                };

                {
                    let mut stream_table = ctx
                        .state_mut()
                        .readable_stream_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if let Some(entry) = stream_table.get_mut(readable_stream_handle as usize) {
                        entry.state = StreamState::Closed;
                    }
                }

                if let Some(promise) = pending {
                    let result = build_reader_result_with_env(ctx, env, true, None);
                    settle_promise(ctx.state_mut(), promise, PromiseSettlement::Fulfill(result));
                }
                finish_writable_stream_close(ctx, writable_stream_handle, close_promise);
                clear_pipe_to(ctx, readable_stream_handle);
            }
            Some(Microtask::ReadableStreamPipeToPump { readable_handle }) => {
                pump_readable_stream_pipe_to_with_env(ctx, env, readable_handle);
            }
            Some(Microtask::AsyncResume {
                fn_table_idx,
                continuation,
                state,
                resume_val,
                completion,
                scope,
            }) => {
                let entered = with_captured_scope_enter(ctx, scope);
                resume_async_function_async(
                    ctx,
                    env,
                    fn_table_idx,
                    continuation,
                    state,
                    resume_val,
                    completion,
                )
                .await;
                with_captured_scope_exit(ctx, entered);
            }
            Some(Microtask::CleanupFinalizationRegistry {
                callback,
                held_value,
            }) => {
                if is_callable_with_env(ctx, env, callback) {
                    let _ = call_host_function_with_args_async(
                        ctx,
                        env,
                        callback,
                        value::encode_undefined(),
                        &[held_value],
                    )
                    .await;
                }
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
            Some(Microtask::WritableStreamSinkWrite {
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
                            "TypeError: WritableStream sink write callback failed".to_string(),
                        );
                        settle_promise(
                            ctx.state_mut(),
                            write_promise,
                            PromiseSettlement::Reject(err),
                        );
                    }
                }
            }
            Some(Microtask::WritableStreamSinkClose {
                callback,
                this_val,
                controller,
                writable_stream_handle,
                close_promise,
            }) => {
                let close_ok = match callback {
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
                if !close_ok {
                    let err = runtime_error_value(
                        ctx.state_mut(),
                        "TypeError: WritableStream sink close callback failed".to_string(),
                    );
                    settle_promise(
                        ctx.state_mut(),
                        close_promise,
                        PromiseSettlement::Reject(err),
                    );
                } else {
                    finish_writable_stream_close(ctx, writable_stream_handle, close_promise);
                }
            }
            None => break,
        }
        if crate::runtime_process::pending_process_exit_signal(ctx.state_mut()).is_some() {
            return;
        }
    }
    crate::runtime_async_fn::recycle_completed_continuations(ctx.state_mut());
    let unhandled: Vec<i64> = {
        let rejections = std::mem::take(
            &mut *ctx
                .state_mut()
                .pending_unhandled_rejections
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        let table = ctx
            .state_mut()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        rejections
            .into_iter()
            .filter_map(|h| {
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
        let msg = render_unhandled_rejection_reason_with_env(ctx, env, reason);
        let line = format!("UnhandledPromiseRejectionWarning: {msg}");
        append_runtime_diagnostic(ctx, &line);
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
    invoke_resolved_callback_async_option(ctx, env, handler, this_val, args).await
}
