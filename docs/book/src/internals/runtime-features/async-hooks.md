# `node:async_hooks` 与 AsyncLocalStorage

这一章说明 `AsyncLocalStorage` 如何通过 `CapturedScope` 跨 await 传播。

## 作用域捕获

wjsm 的 `AsyncLocalStorage` 基于 `CapturedScope` 机制。当 async_hooks 或 AsyncLocalStorage 开启时，调度点（spawn 之前）调用 `capture_completion_scope_from_caller` 捕获当前 scope：

```rust
pub(crate) fn capture_completion_scope_from_caller(
    caller: &Caller<'_, RuntimeState>,
) -> Option<crate::CapturedScope>
```

捕获的 scope 存入 `AsyncHostCompletion` 的 `scope` 字段。fire 时恢复 scope，让 `AsyncLocalStorage` 能跨 await 访问正确的 store。

## 禁止 fire-time current

调度器禁止在 fire 时读取「current」scope——必须在调度/发起时捕获。这是因为 fire 时的执行上下文可能与发起时不同（例如 Worker 线程的回调），直接读取 current 会得到错误的 store。

## Promise reaction 继承

`PromiseThen` 注册 reaction 时，reaction 继承 Promise 创建时的 scope。这是 `AsyncLocalStorage` 能跨 `await` 传播的基础——`await` 本质是 `PromiseThen`，reaction 在恢复时执行，scope 从创建时继承。

## async_hooks 的覆盖范围

async_hooks 在以下位置捕获 scope：

- `runtime_microtask.rs`：微任务调度。
- `runtime_promises.rs`：Promise settle。
- `runtime_node_worker_threads/`：Worker 消息投递。
- `runtime_node_child_process/`：子进程消息回调。
- `runtime_gc/weak_refs.rs`：FinalizationRegistry 回调。
- `heap_context_impl.rs`：堆操作再入。

## 深入了解

- [Promise、微任务与异步调度器](async-scheduler.md)
- [Worker Threads](worker-threads.md)
- [用户侧的异步任务与 Promise](../../user/runtime/async-and-promises.md)
