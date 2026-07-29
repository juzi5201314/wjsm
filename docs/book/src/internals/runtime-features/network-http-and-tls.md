# 网络、HTTP 与 TLS

这一章说明 Fetch API、`node:http` 和 TLS 的实现。

## Fetch API

`crates/wjsm-builtins/src/fetch/` 实现 Web Fetch API：

| 文件 | 内容 |
| --- | --- |
| `core.rs` | fetch 核心，请求发起与响应处理 |
| `request.rs` | Request 对象 |
| `response.rs` | Response 对象 |
| `headers.rs` | Headers 对象 |
| `objects.rs` | fetch 相关构造器 |
| `constructors.rs` | 工厂函数 |
| `resource_timing.rs` | 资源计时 |

`fetch()` 返回 Promise，通过 `AsyncHostCompletion` 的 `Materialize` 闭包驱动。闭包在 scheduler owner 上执行 HTTP 请求，把响应 body 通过 ReadableStream 推送。

## node:http

`node:http` 和 `node:https` 模块提供 Node.js 风格的 HTTP 客户端和服务端 API。实现基于 Fetch 底层能力，包装成 Node.js 的 `http.request` / `http.get` 接口。

## TLS

TLS 通过 Rust 的 TLS 库实现。`node:tls` 模块提供 `connect`、`createServer` 等 API。TLS 连接的底层 socket 与 Fetch 共用 I/O 调度机制。

## Streams 对接

HTTP 响应 body 通过 ReadableStream 暴露。`readable_pipe.rs` 把底层 I/O 资源与 ReadableStream 对接，数据通过 `AsyncHostCompletion` 的 `Materialize` 闭包从 I/O 推入 Stream。

## 深入了解

- [Timer、Event 与 Stream](timers-events-and-streams.md)
- [Promise、微任务与异步调度器](async-scheduler.md)
- [用户侧的文件系统、网络与进程能力](../../user/runtime/system-capabilities.md)
