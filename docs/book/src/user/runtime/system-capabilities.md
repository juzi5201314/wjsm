# 文件系统、网络与进程能力

wjsm 的运行时能力由内置 Node.js 模块和全局对象提供。它们在语义层的全局名单中注册，不需要 `--experimental-*` 之类的开关。完整模块清单和导出项数量见 [Node.js 兼容矩阵](../reference/node-compatibility-matrix.md)，本章按能力维度分组说明。

## 文件系统

`node:fs` 和 `node:fs/promises` 提供同步与异步文件操作：

```js
import { readFileSync, writeFileSync } from "node:fs";
import { readFile } from "node:fs/promises";

const data = readFileSync("input.txt", "utf8");
writeFileSync("output.txt", data.toUpperCase());

const async = await readFile("input.txt", "utf8");
```

| API | 行为 |
| --- | --- |
| `readFileSync` / `writeFileSync` / `appendFileSync` | 同步读写，返回 Buffer 或字符串 |
| `readdirSync` / `mkdirSync` / `rmSync` | 同步目录操作 |
| `statSync` / `existsSync` | 同步文件元信息查询 |
| `readFile` / `writeFile` | 异步版本，返回 Promise |

文件读写受文件系统沙箱约束——进程只能访问 OS 权限允许的路径，wjsm 不在进程内提供额外的 filesystem 沙箱。需要严格隔离时用容器或 chroot 等外部手段。

## 网络

`node:http`、`node:https`、`node:net`、`node:tls` 和 `node:dgram` 覆盖 TCP/UDP/TLS 的客户端与服务端场景。Web Fetch API 也在全局可用：

```js
// Web Fetch
const res = await fetch("https://example.com/api");
console.log(await res.text());

// Node 风格 HTTP 客户端
import http from "node:http";
http.get("http://localhost:3000/health", (res) => {
  res.on("data", (chunk) => console.log(chunk.toString()));
});
```

| 模块 | 覆盖范围 |
| --- | --- |
| `node:http` / `node:https` | `request`、`get`、`createServer` |
| `node:net` | TCP socket 与 server |
| `node:tls` | TLS socket（rustls 实现） |
| `node:dgram` | UDP socket |
| `fetch()` | Web Fetch API，返回 Promise |

HTTP 响应 body 通过 ReadableStream 暴露，底层 I/O 由 runtime 的异步调度器驱动，不需要手动 pump。

> <details><summary>fetch 和 node:http 该用哪个？</summary>
>
> 两者底层共用同一套 I/O 调度，选择取决于你的代码风格：
>
> - **`fetch()`**：Web 标准 API，返回 Promise，适合新项目或与浏览器共享代码的场景。不支持 streaming request body 的某些高级形态。
> - **`node:http`**：Node.js 风格，回调 + Stream，适合已有 Node 代码迁移或需要精细控制连接池的场景。
>
> 两者可以混用。
>
> </details>

## 进程与子进程

`node:child_process` 默认禁用。任何 `spawn`、`exec`、`execFile` 调用在未配置白名单时直接拒绝：

```text
child_process execution is disabled for 'echo'; set WJSM_CHILD_PROCESS_ALLOW to an allowlisted command or '*'
```

设置 `WJSM_CHILD_PROCESS_ALLOW` 放开：

```bash
# 允许特定命令
WJSM_CHILD_PROCESS_ALLOW=git,cargo wjsm run app.ts

# 放开全部（仅限受信任环境）
WJSM_CHILD_PROCESS_ALLOW='*' wjsm run app.ts
```

`fork` 启动另一个 wjsm 进程，通过 IPC 通道通信。`exec` 和 `execFile` 是 `spawn` 的 Promise 包装。

## 全局对象

以下对象在全局可用，无需导入：

| 对象 | 用途 |
| --- | --- |
| `process` | `argv`、`env`、`platform`、`arch`、`exit()`、`cwd()`、`nextTick()`、`versions` |
| `Buffer` | 二进制数据处理 |
| `console` | `log`、`error`、`warn`、`info`、`debug` |
| `performance` | 高精度计时 |
| `structuredClone` | 深拷贝 |
| `queueMicrotask` | 微任务调度 |
| `setImmediate` / `clearImmediate` | 立即回调调度 |
| `atob` / `btoa` | Base64 编解码 |
| `TextEncoder` / `TextDecoder` | 字符串编码转换 |
| `URL` / `URLSearchParams` | WHATWG URL（含 IDN）；与 `node:url` 同引用 |

`process.versions` 同时报告 `node: 22.0.0` 和 `wjsm: 0.1.0`，`process.platform` 和 `process.arch` 报告当前宿主信息。

`fetch` 和 Streams 构造器（`Headers`、`Request`、`Response`、`ReadableStream` 等）在全局名单中，但只能直接调用——取值得到 `undefined`，详见[限制与已知差异](limitations.md)。

## 深入了解

- [Node.js 兼容矩阵](../reference/node-compatibility-matrix.md)
- [文件系统、进程与子进程的宿主实现](../../internals/runtime-features/fs-process-and-child-process.md)
- [网络、HTTP 与 TLS 的实现](../../internals/runtime-features/network-http-and-tls.md)
