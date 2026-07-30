# Node.js 兼容能力

wjsm 内置 24 个 Node.js 核心模块的实现，可用 `node:` 前缀或裸名导入。它们由 wjsm 自己用 JavaScript + 宿主函数实现，不是 Node.js 源码移植，因此覆盖范围是 Node API 的子集。

## 可用模块

| 模块 | 说明 |
| --- | --- |
| `path` | 路径拼接、解析、规范化 |
| `util` | `inspect`、`format`、`promisify` 等 |
| `events` | `EventEmitter` |
| `assert` | 断言函数与 `assert.strict` |
| `url` | URL 解析辅助函数 |
| `querystring` | 查询串编解码 |
| `os` | 平台、架构、CPU、内存信息 |
| `fs` | 同步与回调式文件操作 |
| `fs/promises` | Promise 版文件操作 |
| `crypto` | 摘要、HMAC、随机数、UUID |
| `stream` | Readable/Writable/Transform |
| `http` / `https` | 客户端请求与服务端监听 |
| `net` | TCP socket 与 server |
| `tls` | TLS socket（rustls 实现） |
| `dgram` | UDP socket |
| `zlib` | gzip / deflate / brotli |
| `child_process` | 子进程派生（默认禁用，见下） |
| `worker_threads` | Worker 线程与消息通道 |
| `cluster` | 多进程 IPC |
| `vm` | 隔离上下文执行 |
| `async_hooks` | `AsyncLocalStorage` 与异步上下文追踪 |
| `perf_hooks` | `performance` 计时与观察器 |
| `inspector` | CDP inspector 会话接口 |

导入未列出的模块名会在编译期报错：

```text
Unknown built-in module 'node:not_real'
```

## 导入方式

```js
import path from "node:path";       // 推荐
import { EventEmitter } from "events"; // 裸名同样解析到内置模块
const os = require("node:os");      // CommonJS 入口内可用
```

裸名与 `node:` 前缀解析到同一实现。若 `node_modules` 中存在同名包，内置模块优先。

> <details><summary>内置模块 vs `node_modules` 同名包——优先级谁更高？</summary>
>
> 内置模块永远优先。逻辑链是：解析 specifier 时先查「是否在 `node:` 内置表里」，命中就直接返回内置实现；不命中才走 `node_modules`。
>
> 这意味着：如果你装了 `node_modules/path`（一个 npm 包叫 path），但 wjsm 代码里写 `import x from "path"`，拿到的是 wjsm 的内置实现，不是那个 npm 包。
>
> 为什么这样设计：保证 `import "node:path"` 在所有项目里行为一致——不需要担心某个依赖装上之后 `path` 突然变了一个包。Node.js 早期也踩过这个坑，所以后来加了 `node:` 前缀的明确语义。
>
> </details>

## 默认禁用的能力

`child_process` 默认拒绝执行任何命令：

```text
child_process execution is disabled for 'echo'; set WJSM_CHILD_PROCESS_ALLOW to an allowlisted command or '*'
```

设置 `WJSM_CHILD_PROCESS_ALLOW` 为逗号分隔的命令名，或 `*` 放开全部。文件读写同样受沙箱约束。

## 全局对象

`process`、`Buffer`、`console`、`performance`、`structuredClone`、`queueMicrotask`、`setImmediate`、`atob` / `btoa`、`TextEncoder` / `TextDecoder` 直接可用，无需导入。`process.argv`、`process.env`、`process.platform`、`process.exit` 行为与 Node 对齐，`process.versions` 同时报告 `node` 与 `wjsm` 两个版本号。

## 深入了解

- [Node.js Built-in 模块的组织方式与 JS 封装层](../../internals/runtime-features/node-builtins.md)
- [文件系统、进程与子进程宿主实现](../../internals/runtime-features/fs-process-and-child-process.md)
- [Worker Threads 的线程与消息实现](../../internals/runtime-features/worker-threads.md)
