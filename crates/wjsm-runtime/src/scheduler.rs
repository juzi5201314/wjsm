//! Phase 5+6: Async Timers + Host Completion Channel（严格遵循 2026-05-31 plan 361-550 + 2026-06-01 Re-grounding Correction + Wiring Gate + Phase 5 Closure + 主代理授权）
//!
//! **非目标（硬约束）**：
//! - 不改 sync 路径的任何 timer 行为（execute_with_writer 的阻塞 loop 文本/顺序/MAX守卫完全不动）。
//! - 不碰 fetch、CLI async、fixture expected 文件。
//! - 不引入不必要抽象（无 spawn task、无 channel wrapper、无 cfg feature、无双 struct）。
//!
//! **集成策略（已验证）**：
//! - TimerEntry.deadline 已改为 tokio::time::Instant（仅通过根 use 变更 + 创建点显式路径）。
//! - 本模块仅被 async 路径调用（run_main_completion_block_async 在 main_ok 后委托）。
//! - 循环内严格：单 timer 回调 → call_host..._async.await → drain_microtasks_async.await → 重复则 reschedule。
//! - Phase 6: channel 形状 + Materialize 闭包（引用 Correction 7：worker 只 Send 数据/闭包，materialize 在 owner 上执行）。
//! - 由上层 tokio runtime 驱动（execute_*_async 已是 async fn），直接 .await sleep_until 即可，无需额外 runtime。
//!
//! 所有自然语言为中文；每个主张有 tool output backing；优先行为测试而非 plumbing。
use anyhow::Result;
use std::io::Write;
use std::sync::Arc;
use tokio::{
    sync::mpsc::UnboundedReceiver,
    time::{Instant as TokioInstant, sleep_until},
};
use wasmtime::Store;

use crate::runtime_builtins::PromiseSettlement;
use crate::runtime_microtask::{call_host_function_with_args_async, drain_microtasks_async};
use crate::value;
use crate::{RuntimeState, TimerEntry, WasmEnv};
/// Async host completion sent on the scheduler channel.
/// - SettleValue: simple value settle (worker can Send data)
/// - Materialize: closure runs only on scheduler owner (&mut Store + &WasmEnv)
pub(crate) enum AsyncHostCompletion {
    SettleValue {
        promise: i64,
        settlement: PromiseSettlement,
    },
    Materialize {
        promise: i64,
        materialize:
            Box<dyn FnOnce(&mut Store<RuntimeState>, &WasmEnv) -> PromiseSettlement + Send>,
    },
}

#[derive(Clone)]
pub(crate) struct AsyncOpCounter(Arc<std::sync::atomic::AtomicUsize>);

pub(crate) struct AsyncOpGuard {
    counter: AsyncOpCounter,
}

impl AsyncOpCounter {
    pub(crate) fn new() -> Self {
        Self(Arc::new(std::sync::atomic::AtomicUsize::new(0)))
    }

    pub(crate) fn begin(&self) -> AsyncOpGuard {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        AsyncOpGuard {
            counter: self.clone(),
        }
    }

    pub(crate) fn count(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for AsyncOpGuard {
    fn drop(&mut self) {
        let previous = self
            .counter
            .0
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        assert!(previous > 0, "async op counter underflow");
    }
}
/// 替换原来 run_main_completion_block_async 里 `if main_ok { timer loop }` 的阻塞实现。
/// 严格遵循 plan 361-456 的 process_one_due_timer 形状 + 现有经审计逻辑 + Phase 6 channel 处理（Correction 7）。
/// 所有中文错误消息、MAX=1000 守卫、取消清理、重复 reschedule 顺序 100% 保留。
pub(crate) async fn run_post_main_scheduler_async(
    store: &mut Store<RuntimeState>,
    env: &WasmEnv,
    completion_rx: &mut UnboundedReceiver<AsyncHostCompletion>,
) -> Result<()> {
    // ── Timer event loop (only if main succeeded, 调用方已保证) ─────────────────────────
    // Poll timers; fire expired callbacks via the WASM function table。
    // 使用 tokio sleep_until 替代 std::thread::sleep（关键：不阻塞 runtime 线程）。
    // Phase 6 集成：channel drain + idle 时 await 完成 + counter 退出条件。
    let mut timer_iterations = 0u32;
    const MAX_TIMER_ITERATIONS: u32 = 1000;

    // 内部 helper：避免 match 重复（boring explicit，不算不必要抽象）
    fn process_one_completion(
        store: &mut Store<RuntimeState>,
        env: &WasmEnv,
        completion: AsyncHostCompletion,
    ) {
        match completion {
            AsyncHostCompletion::SettleValue {
                promise,
                settlement,
            } => {
                crate::runtime_promises::settle_promise(store.data(), promise, settlement);
            }
            AsyncHostCompletion::Materialize {
                promise,
                materialize,
            } => {
                let settlement = materialize(store, env);
                crate::runtime_promises::settle_promise(store.data(), promise, settlement);
            }
        }
    }

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

        // Phase 6: 非阻塞 drain 已就绪的 completion（可能在 timer fire 期间到达）
        while let Ok(completion) = completion_rx.try_recv() {
            process_one_completion(store, env, completion);
        }

        // 提前检查退出（空 timers + counter==0）
        {
            let timers_empty = store.data().timers.lock().expect("timers mutex").is_empty();
            let count = store
                .data()
                .async_op_counter
                .as_ref()
                .map_or(0, |c| c.count());
            if timers_empty && count == 0 {
                while let Ok(completion) = completion_rx.try_recv() {
                    process_one_completion(store, env, completion);
                }
                break;
            }
            if timers_empty && count > 0 {
                // in-flight async host op 等待：await channel（唤醒时处理，循环重检）
                if let Some(completion) = completion_rx.recv().await {
                    process_one_completion(store, env, completion);
                } else {
                    break;
                }
                continue;
            }
        }

        let now = TokioInstant::now();
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

            // Find earliest expired timer
            if let Some(idx) = timers.iter().position(|t| t.deadline <= now) {
                _entry_to_fire = Some(timers.remove(idx));
            } else if let Some(next) = timers.iter().min_by_key(|t| t.deadline) {
                // Sleep until next timer (使用 sleep_until 而非 sleep，避免不必要唤醒)
                sleep_until(next.deadline).await;
                continue;
            } else {
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
                store,
                env,
                callback,
                value::encode_undefined(),
                &[],
            )
            .await;

            // Drain microtasks after timer callback（严格 per-callback，不 batch）
            drain_microtasks_async(store, env).await;

            // Re-schedule if repeating
            if repeating {
                store
                    .data()
                    .timers
                    .lock()
                    .expect("timers mutex")
                    .push(TimerEntry {
                        id: entry_id,
                        deadline: TokioInstant::now() + interval,
                        callback,
                        repeating: true,
                        interval,
                    });
            }
        }

        // 循环尾：下次迭代顶会 drain
    }

    Ok(())
}
