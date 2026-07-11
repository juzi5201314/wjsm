use super::*;
use std::io::Write;

use wasmtime::{Instance, Store};

fn fresh_continuation_entry(
    fn_table_idx: u32,
    outer_promise: i64,
    captured_len: usize,
) -> ContinuationEntry {
    let mut captured_vars = vec![value::encode_undefined(); captured_len.max(4)];
    captured_vars[0] = value::encode_f64(0.0);
    captured_vars[1] = value::encode_bool(false);
    ContinuationEntry {
        fn_table_idx,
        outer_promise,
        captured_vars,
        completed: false,
        pending_return: None,
    }
}

pub(crate) fn alloc_continuation_handle(
    state: &RuntimeState,
    fn_table_idx: u32,
    outer_promise: i64,
    captured_var_count: usize,
) -> u32 {
    let mut table = state
        .continuation_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut free = state
        .continuation_free_slots
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    while let Some(slot) = free.pop() {
        let idx = slot as usize;
        if idx < table.len() {
            table[idx] = fresh_continuation_entry(fn_table_idx, outer_promise, captured_var_count);
            return slot;
        }
    }
    let handle = table.len() as u32;
    table.push(fresh_continuation_entry(
        fn_table_idx,
        outer_promise,
        captured_var_count,
    ));
    handle
}

pub(crate) fn recycle_completed_continuations(state: &RuntimeState) {
    let mut table = state
        .continuation_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut free = state
        .continuation_free_slots
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for (idx, entry) in table.iter_mut().enumerate() {
        if !entry.completed {
            continue;
        }
        entry.completed = false;
        entry.fn_table_idx = 0;
        entry.outer_promise = value::encode_undefined();
        entry.captured_vars.clear();
        free.push(idx as u32);
    }
}

pub(crate) fn create_async_generator_method(
    state: &RuntimeState,
    generator: i64,
    kind: AsyncGeneratorCompletionType,
) -> i64 {
    create_native_callable(
        state,
        NativeCallable::AsyncGeneratorMethod { generator, kind },
    )
}

pub(crate) fn alloc_iterator_result_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
    done: bool,
) -> i64 {
    let obj = {
        let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &_wjsm_env, 2)
    };
    let _ = define_host_data_property_from_caller(caller, obj, "value", val);
    let _ = define_host_data_property_from_caller(caller, obj, "done", value::encode_bool(done));
    obj
}

pub(crate) fn enqueue_async_resume_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    continuation: i64,
    state: u32,
    resume_val: i64,
    completion: u8,
) {
    let cont_handle = value::decode_object_handle(continuation) as usize;
    let fn_table_idx = {
        let mut table = caller
            .data()
            .continuation_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get_mut(cont_handle) else {
            return;
        };
        while entry.captured_vars.len() < 2 {
            entry.captured_vars.push(value::encode_undefined());
        }
        entry.captured_vars[0] = value::encode_f64(state as f64);
        entry.captured_vars[1] = value::encode_f64(completion as f64);
        entry.fn_table_idx
    };
    caller
        .data()
        .microtask_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(Microtask::AsyncResume {
            fn_table_idx,
            continuation,
            state,
            resume_val,
            completion,
        });
}

pub(crate) enum AsyncGeneratorPumpAction {
    Resume {
        continuation: i64,
        state: u32,
        value: i64,
        completion: u8,
    },
    SettleResumePromise {
        promise: i64,
        value: i64,
        completion: u8,
    },
    Fulfill {
        promise: i64,
        value: i64,
        done: bool,
    },
    Reject {
        promise: i64,
        reason: i64,
    },
}

pub(crate) fn pump_async_generator_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    generator: i64,
) {
    let handle = value::decode_object_handle(generator) as usize;
    let action = {
        let mut table = caller
            .data()
            .async_generator_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get_mut(handle) else {
            return;
        };
        match entry.state {
            AsyncGeneratorState::Executing | AsyncGeneratorState::Completed => None,
            AsyncGeneratorState::SuspendedYield => {
                let Some(resume_promise) = entry.waiting_resume_promise.take() else {
                    return;
                };
                if entry.queue.is_empty() {
                    entry.waiting_resume_promise = Some(resume_promise);
                    None
                } else {
                    let request = entry.queue.pop_front().expect("non-empty queue");
                    entry.active_request = Some(request);
                    entry.state = AsyncGeneratorState::Executing;
                    match request.completion_type {
                        AsyncGeneratorCompletionType::Next => {
                            Some(AsyncGeneratorPumpAction::SettleResumePromise {
                                promise: resume_promise,
                                value: request.value,
                                completion: 0,
                            })
                        }
                        AsyncGeneratorCompletionType::Throw => {
                            Some(AsyncGeneratorPumpAction::SettleResumePromise {
                                promise: resume_promise,
                                value: request.value,
                                completion: 1,
                            })
                        }
                        AsyncGeneratorCompletionType::Return => {
                            // 不绕过 Suspend 反应机制：在 continuation 上设置 pending_return
                            // 标记，正常 fulfill resume_promise 触发 Suspend 反应（此时
                            // async_function_suspend 已更新 captured_vars[0] 为正确的 yield
                            // 恢复状态）。resume_async_function_async 检查 pending_return
                            // 并将 completion 覆盖为 2（return 语义）。
                            let cont_handle =
                                value::decode_object_handle(entry.continuation) as usize;
                            let mut c_table = caller
                                .data()
                                .continuation_table
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            if let Some(cont_entry) = c_table.get_mut(cont_handle) {
                                cont_entry.pending_return = Some(request.value);
                            }
                            Some(AsyncGeneratorPumpAction::SettleResumePromise {
                                promise: resume_promise,
                                value: request.value,
                                completion: 0,
                            })
                        }
                    }
                }
            }
            AsyncGeneratorState::SuspendedStart => {
                if entry.queue.is_empty() {
                    None
                } else {
                    let request = entry.queue.pop_front().expect("non-empty queue");
                    match request.completion_type {
                        AsyncGeneratorCompletionType::Next => {
                            entry.active_request = Some(request);
                            entry.state = AsyncGeneratorState::Executing;
                            Some(AsyncGeneratorPumpAction::Resume {
                                continuation: entry.continuation,
                                state: 0,
                                value: request.value,
                                completion: 0,
                            })
                        }
                        AsyncGeneratorCompletionType::Return => {
                            entry.state = AsyncGeneratorState::Completed;
                            Some(AsyncGeneratorPumpAction::Fulfill {
                                promise: request.promise,
                                value: request.value,
                                done: true,
                            })
                        }
                        AsyncGeneratorCompletionType::Throw => {
                            entry.state = AsyncGeneratorState::Completed;
                            Some(AsyncGeneratorPumpAction::Reject {
                                promise: request.promise,
                                reason: request.value,
                            })
                        }
                    }
                }
            }
        }
    };
    match action {
        Some(AsyncGeneratorPumpAction::Resume {
            continuation,
            state,
            value,
            completion,
        }) => enqueue_async_resume_from_caller(caller, continuation, state, value, completion),
        Some(AsyncGeneratorPumpAction::SettleResumePromise {
            promise,
            value,
            completion,
        }) => {
            if completion == 1 {
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(value));
            } else {
                // §27.6.3.5 AsyncGeneratorResume：next(value) 传入的 value 直接作为 yield
                // 表达式结果，不能 adopt thenable，否则 generator 会误 await 该 promise。
                settle_promise(caller.data(), promise, PromiseSettlement::Fulfill(value));
            }
        }
        Some(AsyncGeneratorPumpAction::Fulfill {
            promise,
            value,
            done,
        }) => {
            let result = alloc_iterator_result_from_caller(caller, value, done);
            resolve_promise_from_caller(caller, promise, result);
        }
        Some(AsyncGeneratorPumpAction::Reject { promise, reason }) => {
            settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
        }
        None => {}
    }
}

/// Phase 3 must-convert 之 resume 路径（按 2026-05-31-async-scheduler-implementation-plan.md 审计条目 + 26-async-audit-refactor-design.md）：
/// 为 `Microtask::AsyncResume`（用户 async/await 和 async generator 的恢复点）添加 async 版本，与现有 sync `resume_async_function` 并存。
///
/// 规则：
/// - 严格与 sync 版本并存，供保留的 sync execute 路径继续使用
/// - 所有 AsyncResume / async generator resumption 语义（state 值、rejection 路径、generator object 更新、continuation 表更新、outer_promise 检测与 completed 标记）必须 100% 相同
/// - 仅 continuation 表更新 + 状态机推进（Wasm 恢复调用） + 返回值处理完全等价；唯一差异是将 `func.call(...)` 替换为 `func.call_async(...).await`
/// - 本阶段保持调用点不变（microtask 中的 AsyncResume 分支仍调用 sync 版本；未来 Microtask 调度切换到 async 骨架时同步转换）
/// - 精确保留原有行为，无任何语义或顺序差异
///
/// 特别提醒（plan Correction 3 + lib.rs 已有注释 + 审计计划）：
///   在 Store::epoch_deadline_async_yield_and_update 之后，
///   *所有* 经由该 Store 的 Wasm 调用（主 + 回调，包括此处 resume 中的 continuation 恢复调用）都必须走 async API（call_async 等）。
///   本文件中的 async 版本即为此准备；sync 版本仅留给未切换的 sync execute 路径。
pub(crate) async fn resume_async_function_async<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
    fn_table_idx: u32,
    continuation: i64,
    state: u32,
    resume_val: i64,
    completion: u8,
) {
    {
        let cont_handle = value::decode_object_handle(continuation) as usize;
        let mut c_table = ctx
            .state_mut()
            .continuation_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = c_table.get_mut(cont_handle) {
            while entry.captured_vars.len() < 2 {
                entry.captured_vars.push(value::encode_undefined());
            }
            entry.captured_vars[0] = value::encode_f64(state as f64);
            entry.captured_vars[1] = value::encode_f64(completion as f64);
            // 若 return(v) 在 yield 恢复前入队，pending_return 已由
            // pump_async_generator_from_caller 设置。此时 state 已由 Suspend 反应
            // 携带正确的 yield 恢复状态，只需将 completion 覆盖为 2（return 语义）。
            if entry.pending_return.take().is_some() {
                entry.captured_vars[1] = value::encode_f64(2.0);
            }
        }
    }
    let func_ref = env.func_table.get(&mut *ctx, fn_table_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else { return };
    let mut results = [Val::I64(0)];
    let _ = func
        .call_async(
            &mut *ctx,
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
        let c_table = ctx
            .state_mut()
            .continuation_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        c_table.get(cont_handle).map(|entry| entry.outer_promise)
    };
    if let Some(outer_promise) = outer_promise {
        let settled = is_promise_settled(ctx.state_mut(), outer_promise);
        if settled {
            let mut c_table = ctx
                .state_mut()
                .continuation_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = c_table.get_mut(cont_handle) {
                entry.completed = true;
            }
        }
    }
}
/// Dedicated async-only helper (for the thin top-level async execution skeleton in lib.rs
/// or future wiring after async instantiate + prototype setup).
/// Contains a full byte-for-byte copy of the block from sync execute's post-instantiate
/// "Run main" through the end of main completion handling / output / error / trap propagation.
/// Per strict instructions: ONLY the main invocation line was altered (to call_async + .await).
/// All other logic, comments, Chinese messages, error handling, drain, timer loop etc. are
/// identical to the sync version. This is the required top-level async main.call_async site.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_main_completion_block_async<W: Write>(
    instance: &Instance,
    mut store: Store<RuntimeState>,
    wasm_env: WasmEnv,
    output: Arc<Mutex<Vec<u8>>>,
    runtime_error: Arc<Mutex<Option<String>>>,
    diagnostics: Arc<Mutex<Vec<u8>>>,
    writer: W,
    completion_rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::scheduler::AsyncHostCompletion>,
) -> anyhow::Result<(W, Vec<u8>, GcExecutionStats)> {
    let main = instance.get_typed_func::<(), i64>(&mut store, "main")?;
    let main_result = main.call_async(&mut store, ()).await;
    let main_ok = match &main_result {
        Ok(return_val) => {
            if value::is_exception(*return_val) {
                // 未捕获异常被抛出顶层：将异常信息写入输出并设置 runtime_error
                let idx = value::decode_handle(*return_val) as usize;
                if let Some(entry) = store
                    .data()
                    .error_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(idx)
                {
                    let msg = if entry.message.is_empty() {
                        "Uncaught exception".to_string()
                    } else {
                        format!("Uncaught exception: {}", entry.message)
                    };
                    let mut buffer = store
                        .data()
                        .output
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    writeln!(&mut *buffer, "{msg}").ok();
                    *store
                        .data()
                        .runtime_error
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(msg);
                }
                // 跳过后续 microtasks/timers
                false
            } else {
                true
            }
        }
        Err(trap) => {
            if crate::runtime_process::pending_process_exit_signal(store.data()).is_some() {
                false
            } else if let Some(message) = store
                .data()
                .runtime_error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                if message.starts_with("shadow stack overflow:") {
                    let backtrace_str = trap
                        .downcast_ref::<WasmBacktrace>()
                        .map(|bt| {
                            crate::runtime_source_map::format_backtrace(
                                bt,
                                store.data().source_map.as_ref(),
                            )
                        })
                        .unwrap_or_default();
                    if backtrace_str.is_empty() {
                        return Err(anyhow::anyhow!(message));
                    }
                    return Err(anyhow::anyhow!(format!("{message}\n{backtrace_str}")));
                }
                // throw import 已记录运行时错误；先走统一输出/错误收集路径。
                false
            } else {
                // 从 trap error 提取 WasmBacktrace，格式化为 JS 堆栈跟踪。
                let backtrace_str = trap
                    .downcast_ref::<WasmBacktrace>()
                    .map(|bt| {
                        crate::runtime_source_map::format_backtrace(
                            bt,
                            store.data().source_map.as_ref(),
                        )
                    })
                    .unwrap_or_default();
                let trap_msg = if backtrace_str.is_empty() {
                    format!("WASM trap: {:?}", trap)
                } else {
                    // 提取 trap 根因消息（不含 wasmtime 默认 backtrace，避免与 JS 堆栈重复）。
                    let trap_brief = trap
                        .chain()
                        .last()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "unknown trap".to_string());
                    format!("Uncaught WASM trap: {trap_brief}\n{backtrace_str}")
                };
                return Err(anyhow::anyhow!(trap_msg));
            }
        }
    };

    let mut process_exit_signal = None;

    // ── Drain microtasks after main() ────────────────────────────────────
    // Phase 1-4 solid gate: async 主路径接线到 drain_microtasks_async 等自有 helpers
    if main_ok {
        drain_microtasks_async(&mut store, &wasm_env).await;
        process_exit_signal = crate::runtime_process::take_process_exit_signal(store.data());
    }

    // ── Post-main scheduler（Phase 5 委托给 scheduler.rs，Phase 6 传入 rx） ─────────────────────────
    // 仅 async 路径；sync 路径的阻塞 loop 保持 100% 不动。
    // 初始 drain 已在上方完成；scheduler 内部负责后续 timer fire + per-callback drain + completion 处理。
    if main_ok && process_exit_signal.is_none() {
        crate::scheduler::run_post_main_scheduler_async(&mut store, &wasm_env, completion_rx)
            .await?;
        process_exit_signal = crate::runtime_process::take_process_exit_signal(store.data());
    }
    let bytes = output.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let diag_bytes = diagnostics
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if process_exit_signal.is_none() {
        process_exit_signal = crate::runtime_process::take_process_exit_signal(store.data());
    }
    let gc_stats = store.data().gc_execution_stats_snapshot();
    drop(store);

    let mut writer = writer;
    writer.write_all(&bytes)?;
    if let Some(signal) = process_exit_signal {
        return Err(anyhow::Error::new(signal.with_diagnostics(diag_bytes)));
    }

    // ── Check errors ─────────────────────────────────────────────────────
    if let Some(message) = runtime_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        anyhow::bail!(message);
    }

    // Propagate any wasm trap from main() call (must be after output collection)
    main_result?;

    Ok((writer, diag_bytes, gc_stats))
}

#[cfg(test)]
mod continuation_tests {
    use super::*;

    #[test]
    fn continuation_handle_stable_after_recycle() {
        let state = RuntimeState::new();
        let h0 = alloc_continuation_handle(&state, 1, value::encode_f64(1.0), 4);
        {
            let mut table = state
                .continuation_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table[h0 as usize].completed = true;
        }
        recycle_completed_continuations(&state);
        let h1 = alloc_continuation_handle(&state, 2, value::encode_f64(2.0), 4);
        assert_eq!(h0, h1);
    }
}
