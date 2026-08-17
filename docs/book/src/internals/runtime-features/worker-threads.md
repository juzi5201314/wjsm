# Worker Threads

这一章说明 `node:worker_threads` 的实现。

## 模块组织

`crates/wjsm-host-native/src/runtime_node_worker_threads/` 实现 Worker Threads：

| 文件 | 内容 |
| --- | --- |
| `worker.rs` | Worker 类，创建和管理 worker |
| `port.rs` | MessagePort，线程间消息传递 |
| `cluster.rs` | Cluster 模式 |

JS 侧的 polyfill 在 `crates/wjsm-module/builtin_js/node_worker_threads.js`。

## Worker 创建

`new Worker(filename)` 启动一个新的 wjsm 执行实例。worker 与主线程通过 MessagePort 双向通信：

1. 主线程创建 Worker，得到一对 MessagePort。
2. 主线程持有 port A，把 port B 传给 worker。
3. worker 启动后通过 port B 接收消息，通过 `postMessage` 发送。
4. `parentPort` 是 worker 侧的 MessagePort，与主线程的 port A 对应。

## SharedRuntimeState

`SharedRuntimeState` 是跨线程共享的状态，基于 `Arc`。它允许 worker 和主线程共享一些全局信息（如 process 信息）。

`execute_with_writer_shared_agent_options` 接受 `Arc<SharedRuntimeState>`，在 worker 线程中执行。

## 消息调度

MessagePort 的消息通过 `AsyncHostCompletion::HostTask` 投递。`scope` 在投递时捕获（如果 async_hooks 开启），fire 时恢复。这保证 `AsyncLocalStorage` 能跨线程传播。

## 限制

Worker 是独立 agent，有自己的 ManagedHeap 和 GC。worker 之间不共享 JavaScript 对象或 GC handle，只能通过结构化克隆传递消息。`SharedArrayBuffer` 有独立的字节 backing，经 SAB/Atomics 共享，不是对象堆，也不是 Wasm shared memory。

## 深入了解

- [Promise、微任务与异步调度器](async-scheduler.md)
- [`node:async_hooks` 与 AsyncLocalStorage](async-hooks.md)
- [SharedRuntimeState 与 shared buffer](../host-runtime/runtime-state-and-realms.md)
