# Node.js 兼容矩阵

wjsm 内置 24 个 Node.js 模块封装，`node:` 前缀和裸名都能解析。下表的「导出项」是核对时通过静态导入实测得到的键数量，用于判断封装的粗略完整度，不代表逐个 API 与 Node 行为一致。

| 模块 | 导出项 | 备注 |
| --- | --- | --- |
| `path` | 14 | |
| `util` | 10 | 含 `inspect` |
| `events` | 2 | `EventEmitter` 及默认导出 |
| `assert` | 19 | |
| `url` | 9 | 全局 `URL` 不可用，用本模块 |
| `querystring` | 4 | |
| `os` | 14 | |
| `fs` | 20 | 受文件系统沙箱约束 |
| `fs/promises` | 11 | 同上 |
| `crypto` | 7 | |
| `stream` | 8 | |
| `http` | 10 | |
| `net` | 9 | |
| `https` | 10 | |
| `zlib` | 17 | |
| `child_process` | 6 | 默认禁用，需 `WJSM_CHILD_PROCESS_ALLOW` |
| `dgram` | 2 | |
| `tls` | 6 | |
| `worker_threads` | 10 | 上限 32，可用 `WJSM_WORKER_THREADS_MAX` 调整 |
| `inspector` | 3 | |
| `cluster` | 21 | |
| `vm` | 11 | 多 Realm，上限 1024 |
| `async_hooks` | 7 | 含 `AsyncLocalStorage` |
| `perf_hooks` | 13 | |

未列出的模块（如 `readline`、`repl`、`v8`、`module`）没有内置封装，导入 `node:` 前缀形式会报 `Unknown built-in module`。

> <details><summary>「导出项数量」够用吗？</summary>
>
> 不一定够。表里的数字是「裸 specifier 静态导入能拿到的命名导出个数」——比如 `import * as path from "node:path"` 能拿到 14 个键。但每个键下面有什么 API、行为是否与 Node 一致，是另一回事。
>
> 经验上：
>
> - 数字 < 5：核心 API 覆盖（`events`、`dgram` 这种就是几个核心 export）。
> - 数字 5-15：常用 API 覆盖（`http`、`net`、`tls` 之类），能跑大多数用例。
> - 数字 > 15：实现较全（`path`、`os`、`cluster`），但仍要按需验证。
>
> 想确认某个具体 API：直接跑 `wjsm run -e 'import { ... } from "node:xxx"; ...'`。看错误信息比查表快。
>
> </details>

## 全局对象

`process` 可用，`process.versions` 报告 `node: 22.0.0` 与 `wjsm: 0.1.0`，`process.platform`、`process.arch`、`process.argv`、`process.env`、`process.nextTick`、`process.exit` 都可用。

`Buffer`、`TextEncoder`、`TextDecoder`、`structuredClone`、`queueMicrotask`、`atob`、`btoa`、`performance`、`setImmediate`、`clearImmediate` 在全局名单中。

`fetch` 与 Streams 构造器只能直接调用，取值得到 `undefined`，细节见[限制与已知差异](../runtime/limitations.md)。

## 解析优先级

内置模块名优先于 `node_modules` 中的同名包。工程里存在自己的 `path` 包时，`import "path"` 仍然解析到内置封装。

## 深入了解

- [Node.js Built-in 模块的组织方式与新增流程](../../internals/runtime-features/node-builtins.md)
- [worker_threads 的宿主实现与线程模型](../../internals/runtime-features/worker-threads.md)
- [node:vm 的多 Realm 设计](../../internals/runtime-features/node-vm.md)
- [async_hooks 与 AsyncLocalStorage 的上下文所有权](../../internals/runtime-features/async-hooks.md)
