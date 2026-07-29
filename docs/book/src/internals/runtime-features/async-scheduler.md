# Promise、微任务与异步调度器

这一章说明 wjsm 的异步调度器如何驱动 Promise 和微任务。

## 统一异步执行模型

wjsm 的异步调度器遵循 2026-06-02 统一异步执行模型计划。核心是 `scheduler.rs` 模块，它驱动 timer、host completion 和微任务。

调度器有两种模式：

- **sync 模式**：`execute_with_writer` 的阻塞 loop，timer 用 `std::thread::sleep`，微任务在每个宏任务后 drain。文本、顺序、MAX 守卫完全不变。
- **async 模式**：`execute_*_async` 系列，由上层 tokio runtime 驱动，timer 用 `sleep_until`，host completion 通过 channel 传递。

## AsyncHostCompletion channel

`AsyncHostCompletion` 是 scheduler channel 上的消息，有三种变体：

| 变体 | 用途 |
| --- | --- |
| `SettleValue` | 简单值 settle（worker 只 Send 数据） |
| `Materialize` | 闭包在 scheduler owner 上执行（`&mut Store + &WasmEnv`） |
| `HostTask` | 非 Promise 副作用（MessagePort 投递、Worker lifecycle 事件） |

每条消息携带 `scope: Option<CapturedScope>`——在调度/发起时捕获（hooks 或 AsyncLocalStorage 开启时），fire 时恢复，禁止 fire-time current。

## 微任务

`drain_microtasks_async` 在每个宏任务后 drain 微任务队列。Promise 的 then/reject/resolve reaction、`queueMicrotask` 的回调、`FinalizationRegistry` 的回调都在这里执行。

## 再入

host→host 重入通过 `cached_wasm_env` 处理。`Caller::get_export` 在纯 host 调用链上不可用，所以 `WasmEnv` 缓存在 `RuntimeState`，重入时直接读取。

## 深入了解

- [`node:async_hooks` 与 AsyncLocalStorage](async-hooks.md)
- [Timer、Event 与 Stream](timers-events-and-streams.md)
- [统一异步执行模型计划文档](../reference/adr-index.md)
