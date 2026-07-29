# Timer、Event 与 Stream

这一章说明 `setTimeout`、事件循环和 Web Streams 的实现。

## Timer

`TimerEntry` 记录定时器信息，`deadline` 是 `tokio::time::Instant`。sync 模式用 `std::thread::sleep`，async 模式用 `sleep_until`。

`__good_color` 和 `__barrier_buf_ptr` 等 env global 不参与 timer，timer 完全在宿主侧管理。timer 回调作为微任务调度，在 `drain_microtasks_async` 中执行。

## 事件循环

wjsm 的事件循环是简化的：没有 Node.js 的四个阶段（timers / pending / poll / check），只有宏任务 + 微任务两层。

- **宏任务**：timer 回调、I/O 回调、MessagePort 消息。
- **微任务**：Promise reaction、`queueMicrotask`、`FinalizationRegistry` 回调。

每个宏任务后 drain 微任务队列，直到队列为空。

## Web Streams

`crates/wjsm-builtins/src/streams/` 实现 Web Streams API：

| 文件 | 内容 |
| --- | --- |
| `readable_stream.rs` | ReadableStream |
| `writable_stream.rs` | WritableStream |
| `transform_stream.rs` | TransformStream |
| `queuing_strategy.rs` / `queuing.rs` | 队列策略 |
| `byte_source.rs` | 字节源 |
| `readable_pipe.rs` | 与 fetch body 的管道对接 |

Streams 通过 `AsyncHostCompletion` 的 `Materialize` 闭包驱动——闭包在 scheduler owner 上执行，读取底层 I/O 资源，把数据推入 Stream 的内部队列。

## 深入了解

- [Promise、微任务与异步调度器](async-scheduler.md)
- [网络、HTTP 与 TLS](network-http-and-tls.md)
- [用户侧的异步任务与 Promise](../../user/runtime/async-and-promises.md)
