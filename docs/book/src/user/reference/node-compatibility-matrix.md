# Node.js 兼容矩阵

wjsm 内置 43 个 Node.js 模块封装，`node:` 前缀和裸名都能解析到同一封装（ESM `import` 与 CJS `require` 均可）。下表的「导出项」是核对时通过静态导入实测得到的键数量，用于判断封装的粗略完整度，不代表逐个 API 与 Node 行为一致。

| 模块 | 导出项 | 备注 |
| --- | --- | --- |
| `path` | 14 | |
| `path/posix` | 12 | 与 `path.posix` 同一对象 |
| `path/win32` | 12 | 与 `path.win32` 同一对象 |
| `util` | 10 | 含 `inspect` |
| `util/types` | 9 | 与 `util.types` 同一对象；只含可精确判定的品牌检查 |
| `events` | 2 | `EventEmitter` 及默认导出 |
| `assert` | 19 | |
| `assert/strict` | 19 | `equal`/`deepEqual` 等映射到严格变体 |
| `buffer` | 2 | 导出全局 `Buffer` 与 `transcode` |
| `url` | 9 | 含 IDN；全局 `URL` / `URLSearchParams` 可用 |
| `querystring` | 4 | |
| `os` | 14 | |
| `fs` | 20 | 受文件系统沙箱约束 |
| `fs/promises` | 11 | 同上 |
| `crypto` | 7 | |
| `stream` | 8 | |
| `stream/web` | 6 | re-export 既有 Web Streams：`ReadableStream` / `WritableStream` / `TransformStream` / 两种 QueuingStrategy；BYOB reader、`TextEncoderStream` 等未提供 |
| `http` | 10 | |
| `net` | 9 | |
| `https` | 10 | |
| `zlib` | 17 | |
| `child_process` | 6 | 默认禁用，需 `WJSM_CHILD_PROCESS_ALLOW` |
| `dgram` | 2 | |
| `tls` | 6 | |
| `worker_threads` | 10 | 每 agent 独立堆；无 `WJSM_WORKER_THREADS_MAX` |
| `inspector` | 3 | |
| `cluster` | 21 | |
| `vm` | 11 | 多 Realm，共用同一 ManagedHeap |
| `async_hooks` | 7 | 含 `AsyncLocalStorage` |
| `perf_hooks` | 13 | |
| `string_decoder` | 1 | `StringDecoder`，支持 utf8/utf16le/base64/latin1/hex/ascii 流式解码 |
| `timers` | 7 | 含 `promises` 属性 |
| `timers/promises` | 4 | `setTimeout`/`setImmediate`/`setInterval`/`scheduler` |
| `punycode` | 6 | RFC 3492 完整实现（Node 中已弃用） |
| `process` | 21 | 默认导出即全局 `process` |
| `console` | 7 | 默认导出即全局 `console`，含 `Console` 类 |
| `constants` | 13 | `os.constants` 与 `fs.constants` 摊平（Node 中已弃用） |
| `diagnostics_channel` | 5 | `channel`/`subscribe`/`unsubscribe`/`hasSubscribers`/`Channel` |
| `module` | 7 | `createRequire`（与 CJS 内建 `require` 同一宿主实现）/`builtinModules`（来自注册表）/`isBuiltin`/legacy `Module` 构造器 |
| `readline` | 8 | `createInterface`/`Interface`/行事件/`question` 与光标操作（`cursorTo`/`moveCursor`/`clearLine`/`clearScreenDown`）；`promises` 属性同 `readline/promises`；`emitKeypressEvents` 依赖终端逐键流，未提供 |
| `readline/promises` | 4 | `createInterface`（`question` 返回 Promise）与 `Readline` 光标批量队列（`commit`/`rollback`） |
| `tty` | 4 | `isatty` 真实 fd 探测；`ReadStream`/`WriteStream` 仅形状（stdio 不挂接原始终端流） |
| `v8` | 10 | `getHeapStatistics` 数据与 `process.memoryUsage` 同源（真实 GC 计量，未计量维度报 0）；`serialize` 等抛「not implemented」 |

未列出的模块（如 `repl`、`dns`、`http2`）没有内置封装，导入 `node:` 前缀形式会报 `Unknown built-in module`。`readline` 曾因宿主 `process.stdin` 只有 `on`/`resume` 空实现而被明确排除；现在 `process.stdin` 支持管道输入的真实按块读取（flowing/paused、`read(size)`、`setEncoding('utf8')`、异步迭代），`readline` 与 `readline/promises` 已随之提供（TTY 逐键交互输入仍不在范围，见 limitations）。`dns`、`http2` 涉及真实网络协议栈，不在当前范围。

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

`fetch`、`Headers`、`Request`、`Response`、`ReadableStream`、`WritableStream`、`TransformStream`、`AbortController`（以及 `AbortSignal`、`EventTarget`、`Event`）是全局对象上真实的自有数据属性：own descriptor 与浏览器 / WebIDL 一致（`{writable, enumerable: false, configurable}`，`fetch` 方法额外 enumerable），可取值传递、赋值 / `delete` / `defineProperty` 重定义全按普通属性语义生效，删除后裸标识符读取抛 `ReferenceError`；`typeof` 为 `"function"`、实例可 `instanceof`，`name` / `length` 与 Node v22 一致。

## 解析优先级

内置模块名优先于 `node_modules` 中的同名包。工程里存在自己的 `path` 包时，`import "path"` 仍然解析到内置封装。

## 深入了解

- [Node.js Built-in 模块的组织方式与新增流程](../../internals/runtime-features/node-builtins.md)
- [worker_threads 的宿主实现与线程模型](../../internals/runtime-features/worker-threads.md)
- [node:vm 的多 Realm 设计](../../internals/runtime-features/node-vm.md)
- [async_hooks 与 AsyncLocalStorage 的上下文所有权](../../internals/runtime-features/async-hooks.md)
