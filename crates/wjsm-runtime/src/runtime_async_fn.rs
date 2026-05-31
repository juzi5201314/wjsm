use super::*;
use std::io::Write;
use std::time::Instant;
use wasmtime::{Instance, Store};

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
    is_rejected: bool,
) {
    let cont_handle = value::decode_object_handle(continuation) as usize;
    let fn_table_idx = {
        let mut table = caller
            .data()
            .continuation_table
            .lock()
            .expect("continuation table mutex");
        let Some(entry) = table.get_mut(cont_handle) else {
            return;
        };
        while entry.captured_vars.len() < 2 {
            entry.captured_vars.push(value::encode_undefined());
        }
        entry.captured_vars[0] = value::encode_f64(state as f64);
        entry.captured_vars[1] = value::encode_bool(is_rejected);
        entry.fn_table_idx
    };
    caller
        .data()
        .microtask_queue
        .lock()
        .expect("microtask queue mutex")
        .push_back(Microtask::AsyncResume {
            fn_table_idx,
            continuation,
            state,
            resume_val,
            is_rejected,
        });
}

pub(crate) enum AsyncGeneratorPumpAction {
    Resume {
        continuation: i64,
        state: u32,
        value: i64,
        is_rejected: bool,
    },
    SettleResumePromise {
        promise: i64,
        value: i64,
        is_rejected: bool,
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
            .expect("async generator table mutex");
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
                                is_rejected: false,
                            })
                        }
                        AsyncGeneratorCompletionType::Throw => {
                            Some(AsyncGeneratorPumpAction::SettleResumePromise {
                                promise: resume_promise,
                                value: request.value,
                                is_rejected: true,
                            })
                        }
                        AsyncGeneratorCompletionType::Return => {
                            Some(AsyncGeneratorPumpAction::Fulfill {
                                promise: request.promise,
                                value: request.value,
                                done: true,
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
                                is_rejected: false,
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
            is_rejected,
        }) => enqueue_async_resume_from_caller(caller, continuation, state, value, is_rejected),
        Some(AsyncGeneratorPumpAction::SettleResumePromise {
            promise,
            value,
            is_rejected,
        }) => {
            if is_rejected {
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(value));
            } else {
                resolve_promise_from_caller(caller, promise, value);
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

pub(crate) fn resume_async_function<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    fn_table_idx: u32,
    continuation: i64,
    state: u32,
    resume_val: i64,
    is_rejected: bool,
) {
    {
        let cont_handle = value::decode_object_handle(continuation) as usize;
        let mut c_table = ctx
            .state_mut()
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
    let func_ref = env.func_table.get(&mut *ctx, fn_table_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else { return };
    let mut results = [Val::I64(0)];
    let _ = func.call(
        &mut *ctx,
        &[
            Val::I64(continuation),
            Val::I64(resume_val),
            Val::I32(0),
            Val::I32(0),
        ],
        &mut results,
    );
    let cont_handle = value::decode_object_handle(continuation) as usize;
    let outer_promise = {
        let c_table = ctx
            .state_mut()
            .continuation_table
            .lock()
            .expect("continuation table mutex");
        c_table.get(cont_handle).map(|entry| entry.outer_promise)
    };
    if let Some(outer_promise) = outer_promise {
        let settled = is_promise_settled(ctx.state_mut(), outer_promise);
        if settled {
            let mut c_table = ctx
                .state_mut()
                .continuation_table
                .lock()
                .expect("continuation table mutex");
            if let Some(entry) = c_table.get_mut(cont_handle) {
                entry.completed = true;
            }
        }
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
pub(crate) async fn resume_async_function_async(
    store: &mut Store<RuntimeState>,
    env: &WasmEnv,
    fn_table_idx: u32,
    continuation: i64,
    state: u32,
    resume_val: i64,
    is_rejected: bool,
) {
    {
        let cont_handle = value::decode_object_handle(continuation) as usize;
        let mut c_table = store
            .data_mut()
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
    let func_ref = env.func_table.get(&mut *store, fn_table_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else { return };
    let mut results = [Val::I64(0)];
    // 关键：同步版本用 func.call(...)，async 版本必须替换为 func.call_async(...).await
    // 这是保证 async Store 在 yield_and_update 后不触发 Wasmtime sync call 拒绝的关键转换点。
    let _ = func
        .call_async(
            &mut *store,
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
        let c_table = store
            .data_mut()
            .continuation_table
            .lock()
            .expect("continuation table mutex");
        c_table.get(cont_handle).map(|entry| entry.outer_promise)
    };
    if let Some(outer_promise) = outer_promise {
        let settled = is_promise_settled(store.data_mut(), outer_promise);
        if settled {
            let mut c_table = store
                .data_mut()
                .continuation_table
                .lock()
                .expect("continuation table mutex");
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
pub(crate) async fn run_main_completion_block_async<W: Write>(
    instance: &Instance,
    mut store: Store<RuntimeState>,
    wasm_env: WasmEnv,
    output: Arc<Mutex<Vec<u8>>>,
    runtime_error: Arc<Mutex<Option<String>>>,
    writer: W,
) -> anyhow::Result<W> {
    // ── Run main ──
    let main = instance.get_typed_func::<(), i64>(&mut store, "main")?;
    let main_result = main.call_async(&mut store, ()).await;
    let main_ok = match main_result {
        Ok(return_val) => {
            if value::is_exception(return_val) {
                // 未捕获异常被抛出顶层：将异常信息写入输出并设置 runtime_error
                let idx = value::decode_handle(return_val) as usize;
                if let Some(entry) = store
                    .data()
                    .error_table
                    .lock()
                    .expect("error_table mutex")
                    .get(idx)
                {
                    let msg = if entry.message.is_empty() {
                        "Uncaught exception".to_string()
                    } else {
                        format!("Uncaught exception: {}", entry.message)
                    };
                    let mut buffer = store.data().output.lock().expect("output mutex");
                    writeln!(&mut *buffer, "{msg}").ok();
                    *store
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime_error mutex") = Some(msg);
                }
                // 跳过后续 microtasks/timers
                false
            } else {
                true
            }
        }
        Err(ref trap) => {
            if store
                .data()
                .runtime_error
                .lock()
                .expect("runtime_error mutex")
                .is_some()
            {
                // throw import 已经记录了 JS 层异常；先走统一输出/错误收集路径。
                false
            } else {
                return Err(anyhow::anyhow!("WASM trap: {:?}", trap));
            }
        }
    };

    // ── Drain microtasks after main() ────────────────────────────────────
    // Phase 1-4 solid gate: async 主路径接线到 drain_microtasks_async 等自有 helpers
    if main_ok {
        drain_microtasks_async(&mut store, &wasm_env).await;
    }

    // ── Timer event loop (only if main succeeded) ─────────────────────────
    // Poll timers; fire expired callbacks via the WASM function table.
    if main_ok {
        let mut timer_iterations = 0u32;
        const MAX_TIMER_ITERATIONS: u32 = 1000;
        loop {
            timer_iterations += 1;
            if timer_iterations > MAX_TIMER_ITERATIONS {
                writeln!(
                    store.data().output.lock().expect("output mutex"),
                    "Internal error: timer event loop exceeded max iterations"
                )
                .ok();
                break;
            }
            let now = Instant::now();
            let mut _entry_to_fire: Option<TimerEntry> = None;

            {
                let mut timers = store.data().timers.lock().expect("timers mutex");
                let mut cancelled = store
                    .data()
                    .cancelled_timers
                    .lock()
                    .expect("cancelled_timers mutex");

                // Remove cancelled timers
                timers.retain(|t| !cancelled.contains(&t.id));
                cancelled.clear();

                if timers.is_empty() {
                    break;
                }

                // Find earliest expired timer
                if let Some(idx) = timers.iter().position(|t| t.deadline <= now) {
                    _entry_to_fire = Some(timers.remove(idx));
                } else {
                    // Sleep until next timer
                    let next = timers.iter().min_by_key(|t| t.deadline).unwrap().deadline;
                    let dur = next.saturating_duration_since(Instant::now());
                    if !dur.is_zero() {
                        std::thread::sleep(dur);
                    }
                    continue;
                }
            }

            if let Some(entry) = _entry_to_fire {
                let callback = entry.callback;
                let repeating = entry.repeating;
                let interval = entry.interval;
                let entry_id = entry.id;

                // 定时器回调按宿主 API 语义以 this=undefined、零参数调用。
                call_host_function_with_args_async(
                    &mut store,
                    &wasm_env,
                    callback,
                    value::encode_undefined(),
                    &[],
                ).await;

                // Drain microtasks after timer callback
                drain_microtasks_async(&mut store, &wasm_env).await;
                // Re-schedule if repeating
                if repeating {
                    store
                        .data()
                        .timers
                        .lock()
                        .expect("timers mutex")
                        .push(TimerEntry {
                            id: entry_id,
                            deadline: Instant::now() + interval,
                            callback,
                            repeating: true,
                            interval,
                        });
                }
            }
        }
    }
    // ── Collect output ────────────────────────────────────────────────────
    let bytes = output
        .lock()
        .expect("runtime output buffer mutex should not be poisoned")
        .clone();
    drop(store);

    let mut writer = writer;
    writer.write_all(&bytes)?;

    // ── Check errors ─────────────────────────────────────────────────────
    if let Some(message) = runtime_error.lock().expect("runtime error mutex").clone() {
        anyhow::bail!(message);
    }

    // Propagate any wasm trap from main() call (must be after output collection)
    main_result?;

    Ok(writer)
}
