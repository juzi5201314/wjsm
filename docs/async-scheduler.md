# Async Scheduler 设计与嵌入契约（2026-05-31 实现，2026-06-02 统一异步执行模型）

**状态**：调度器、async-only 公共 API、全部宿主 import 异步覆盖、sync 辅助函数清理及 eval 解释器 async 孪生已全部完成。本文档反映最终交付状态。

本文档是 `wjsm-runtime` 异步执行路径的权威说明，供未来维护者和嵌入者使用。

## 核心架构

- 单一 scheduler owner 持有 `Store<RuntimeState>`，使用独占 `&mut Store` 轮询。
- Wasmtime async 入口（`instantiate_async`、`call_async`、`func_wrap_async`）在启用 epoch yielding 后必须全程使用。
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
- 当所有定时器被取消且队列为空时，调度器不得对空队列 `unwrap`（`scheduler.rs` 使用 `else if let Some(next)` 分支）。

此模型保证 JavaScript run-to-completion 语义，并避免在主 future 持有 Store 时并发访问。
async 函数/TLA 的 `$state` dispatch 依赖 IR `Terminator::Switch` 与 `Instruction::Suspend` 的交叉编译；后端在 `compile_switch_case` 中必须始终发射各 case 入口（见 `crates/wjsm-backend-wasm/tests/async_switch_compile.rs`）。


## Worker 边界（Host Completion Materialization）

`AsyncHostCompletion`：

- `SettleValue { promise, settlement }`：简单值 settlement。
- `Materialize { promise, materialize: Box<dyn FnOnce(&mut Store, &WasmEnv) -> PromiseSettlement + Send> }`：闭包在 scheduler owner 上执行，可安全分配 heap 对象。

`AsyncOpGuard` / `AsyncOpCounter` 用于 in-flight 跟踪。调度器仅在 `counter.count() == 0` 且 receiver 为空时退出。

### HTTP body pull workers

`ReadableStream` byte streams backed by an HTTP `Response` spawn one tokio task per `reader.read()` call:

```text
reader.read(view)
  -> take reqwest::Response from HttpResponseEntry.response
  -> mark HttpResponseEntry.pending_read_promise = promise
  -> AsyncOpGuard::begin()
  -> tokio::spawn(async move {
       let _guard: Option<AsyncOpGuard> = guard;
       let outcome = response.chunk().await;
       tx.send(Materialize { promise, materialize });
     })
```

**要求**：每个 spawn 出去的 task 必须持有 `AsyncOpGuard`，否则调度器可能在 `response.chunk().await` 期间观察到 `count == 0` 而退出。HTTP `fetch()` 的 inline await (`perform_http_fetch.await`) 同样需要 guard，因为它可能在 post-main scheduler 启动前跨越 `main.call_async` 的返回边界。

`Materialize` 闭包在 scheduler owner 上执行：put `response` 回 `HttpResponseEntry`（成功路径），overflow bytes 进入 `pending_bytes`，并 fulfill 读取 promise。失败路径记录 `error` 并 reject。

**绝对禁止**：worker 线程创建 JS 对象句柄或直接操作 Store。

## 公共 API（async-only，2026-06-02）

`wjsm-runtime` 仅导出异步执行入口：

```rust
pub async fn execute(wasm_bytes: &[u8]) -> anyhow::Result<()>;
pub async fn execute_with_writer<W: std::io::Write>(
    wasm_bytes: &[u8],
    writer: W,
) -> anyhow::Result<W>;
```

- 已删除同步 `execute` / `execute_with_writer` 及 crate 内 Tokio `block_on` 包装。
- 同步命令行边界由 **`wjsm-cli`** 创建 `tokio::runtime::Runtime` 并 `block_on(wjsm_runtime::execute(...))`。
- **不得**在已有 Tokio runtime 内部对同一 Store 再嵌套 `block_on`（易 panic/死锁）。
- `execute_with_writer` 的 future 对某些 `W` 可能不是 `Send`；跨线程 spawn 时选用 `Send` writer 或保持本地。

## Linker 注册模式

`register_linker` 对可能 re-enter Wasm 的 import 采用 `linker.func_wrap_async` 注册；纯内存/状态类 import 仍用 `Func::wrap`，且不得调用 `func.call` / `call_wasm_callback`。

所有同步 re-entry 回调块已删除；`skip_wasm_reentrant` 机制随 Task 16 退役。

## CLI 迁移策略

- CLI 通过本地 Tokio runtime + `block_on` 调用 `wjsm_runtime::execute`，行为与原先同步 wrapper 等价。
- CLI 原生 `#[tokio::main]`（watch、signal 等）仍为后续独立计划。

## 验证与测试

已交付的针对性测试覆盖：

1. `timer_timing`（10/10）— 含 clearTimeout、setInterval 回调内 clear。
2. `async_scheduler`（11/11）— 含 microtask 顺序、timer drain、fetch promise、async 异常、数组回调、Function call/apply、microtask/timer 回调、JSON.parse reviver、Proxy/Reflect trap、Proxy 内部 trap、eval 直接/间接/NativeCallable。
3. `async_reentry_audit`（1/1，STRICT）— `async_reentry_audit_forbidden_sync_patterns` 在 `STRICT_AUDIT = true` 下通过，源码中已不存在 async Store 可达路径的 sync re-entry。
4. E2E fixture：全 workspace 970 tests passed，`.expected` 零变更。

运行：

```bash
cargo nextest run --workspace
cargo nextest run -p wjsm-runtime -E 'test(async_scheduler) or test(async_reentry)'
```
## Startup Snapshot 兼容性

Startup snapshot（默认开启，`WJSM_STARTUP_SNAPSHOT=0` 关闭）不保存 scheduler、worker、async host completion channel/counter 状态。Snapshot capture 在 bootstrap 后、用户 JS 前执行，此时 scheduler 尚未启动、无 timer/microtask/promise/continuation 残留（capture 时断言所有 side table 为空）。Restore 仅在 scheduler owner 上执行，恢复后由 `main()` 正常驱动调度器。详见 `docs/adr/0003-startup-snapshot-boundary.md`。

## 后续工作

- 真实 async fetch、AbortController、CLI watch 原生 async（独立计划）。

## 参考
- 统一异步执行计划：`docs/aegis/plans/2026-06-02-unified-async-execution-model.md`
- 设计规格：`docs/aegis/specs/2026-06-02-unified-async-execution-model-design.md`
- 调度器实现计划：`docs/aegis/plans/2026-05-31-async-scheduler-implementation-plan.md`
- 调度器设计规格：`docs/aegis/specs/2026-05-31-async-scheduler-redesign-design.md`

**维护者提示**：任何触碰 Store 的异步路径变更，须更新本文档并运行 `timer_timing` + `async_scheduler` + 相关 `async_reentry` 测试；fixture `.expected` 不得因“修测试”而擅自改动，除非行为经 spec 确认变更。