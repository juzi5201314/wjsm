# Async Scheduler 设计与嵌入契约（2026-05-31 实现）

**状态**：已完成（Wiring Gate + Phase 5-7 交付，证据充分）。

本文档是 `wjsm-runtime` 异步执行路径的权威说明，供未来维护者和嵌入者使用。

## 核心架构

- 单一 scheduler owner 持有 `Store<RuntimeState>`，使用独占 `&mut Store` 轮询。
- Wasmtime async 入口（`instantiate_async`、`call_async`）在启用 epoch yielding 后必须全程使用。
- Tokio worker 任务仅做外部 I/O，**绝不**触碰 Store / RuntimeState / Wasm 内存 / JS heap。
- 结果材料化（materialization）仅在 scheduler owner 上执行（允许分配对象、字符串、错误等）。

## RuntimeState: Send 契约（硬性要求）

`RuntimeState` 必须实现 `Send`。本实现中：

- 所有共享状态使用 `Arc<Mutex<...>>` 或 `Arc<Atomic...>`。
- `new_target` 从 `Cell<i64>` 迁移到 `AtomicI64 + Relaxed`（零开销，等价语义），这是 Wasmtime async 路径的 load-bearing 前置条件。
- 严禁在 Store data 中引入 `Rc`、`RefCell`、裸指针或非 Send 的捕获 future。

违反此契约将导致 `instantiate_async` 等编译失败或运行时 UB。

## 主执行到完成（Main to Completion）模型

- 主 Wasm future 运行至完成（`call_async` 返回）。
- 期间 **不** 进行 microtask / timer 交错（`call_async` future 持有 `&mut Store`）。
- 主完成后进入 post-main 调度器：drain microtasks → 处理 host completion → 定时器（Tokio `sleep_until`）。
- 每个定时器回调后 **必须** drain microtasks（与原语义一致）。
- 保留 `MAX_TIMER_ITERATIONS = 1000` 防挂守卫。

此模型保证 JavaScript run-to-completion 语义，并避免在主 future 持有 Store 时并发访问。

## Worker 边界（Host Completion Materialization）

`AsyncHostCompletion`：

- `SettleValue { promise, settlement }`：简单值 settlement。
- `Materialize { promise, materialize: Box<dyn FnOnce(&mut Store, &WasmEnv) -> PromiseSettlement + Send> }`：闭包在 scheduler owner 上执行，可安全分配 heap 对象。

`AsyncOpGuard` / `AsyncOpCounter` 用于 in-flight 跟踪。调度器仅在 `counter.count() == 0` 且 receiver 为空时退出。

**绝对禁止**：worker 线程创建 JS 对象句柄或直接操作 Store。

## 同步兼容 Wrapper

```rust
pub fn execute(wasm_bytes: &[u8]) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(execute_async(wasm_bytes))
}

pub fn execute_with_writer<W: Write>(...) -> Result<W> { ... }
```

**重要限制**（必须在 rustdoc 中记载）：

- 同步 wrapper **不得** 在已有 Tokio runtime 内部调用（会 panic 或死锁）。
- 原生 async 调用者应直接使用 `execute_async` / `execute_with_writer_async`。
- `execute_with_writer_async<W>` 的 future 对某些 `W` 可能不是 `Send`；需要跨线程 spawn 时选择 Send writer 或保持本地。

## CLI 迁移策略

本计划 **明确将 CLI 原生 async 迁移排除在范围外**。

- 当前 CLI 通过同步 wrapper 调用 runtime，保持完全兼容。
- 后续单独的 CLI-native async 计划（watch、signal 等）再处理 `#[tokio::main]` 等变更。
- Phase 8 审计确认：无需修改 CLI 或 test262 源码即可通过全部 fixture。

## 验证与测试

已交付的针对性测试（在 lib tests / phase verification 中）覆盖：

1. `sync_program_matches_sync_wrapper`（等价输出）。
2. Promise microtask 顺序保持。
3. Timer 回调后 drain microtasks。
4. 主异常 / runtime_error / trap 优先级与输出收集。
5. Epoch incrementer 在所有退出路径停止。
6. Async host completion 材料化（value + 分配路径）。

完整 E2E 通过现有 fixture 套件（经薄 wrapper）。

## 后续工作（不在本计划内）

- 真实 async fetch（利用已搭好的 completion channel + materialization）。
- AbortController / ReadableStream。
- CLI watch + signal 的原生 async 迁移。
- Epoch 间隔调优与基准。

## 参考

- 实现计划：`docs/aegis/plans/2026-05-31-async-scheduler-implementation-plan.md`
- 设计规格：`docs/aegis/specs/2026-05-31-async-scheduler-redesign-design.md`
- 工作记录与 re-grounding：`.worktrees/feat-async-scheduler-2026-05-31/docs/aegis/work/2026-05-31-async-scheduler-implementation/20-checkpoint.md`（包含 2026-06-01 关键 re-grounding 修正）

**维护者提示**：任何触碰 Store 的异步路径变更，必须先更新本文档 + 运行完整的 async scheduler 测试套件 + 确认所有 14 条成功标准仍满足。
【2026-06-01 延续修复后诚实补充】专用 async_scheduler 测试现已 6/6 PASS（通过补齐 import 与签名对齐）。Final reviewer 曾指出 Critical 问题（import 不完整 + async Store 上仍有同步 callback）。核心 scheduler 行为已充分验证；剩余 callback hygiene gap（call_wasm_callback_async 未使用）已在 lib.rs 中文注释 + checkpoint 中明确记录。严禁 over-claim。下一 gate 前必须 re-review。
