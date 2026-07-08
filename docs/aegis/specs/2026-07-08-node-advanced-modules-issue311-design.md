# Node 高级内置模块设计（issue #311）

- 日期：2026-07-08
- 状态：已按用户本轮“无需审批 spec”授权进入实现
- 关联 issue：#311
- 输入：issue #311 正文与审查意见、`AGENTS.md` runtime/module 架构约束、`docs/aegis/specs/2026-07-07-runtime-module-loading-design.md`、`docs/aegis/specs/2026-07-07-package-management-design.md`、当前 `builtin_js` / `runtime_node_*` / CLI 代码

## 1. 背景与当前事实

issue #311 目标是补齐 Node.js 生态常用的高级内置模块与 CLI tooling。当前事实：

| 领域 | 当前实现 | 缺口 |
|---|---|---|
| WHATWG Streams | runtime 已实现 Web `ReadableStream` / `WritableStream` / `TransformStream` | Node.js `stream` 模块完全不同，尚未实现 |
| fetch | runtime 已实现 `fetch` / `Headers` / `Request` / `Response` | `http.request/get` 兼容封装尚未实现 |
| events | `node_events.js` 已实现 `EventEmitter` 基础能力 | `stream` / `child_process` 可复用 |
| Buffer | runtime 提供全局 `Buffer` 与常用静态/实例方法 | zlib / child_process 输出可直接返回 Buffer |
| fs/crypto builtin | JS builtin + `__wjsm_node_*` host object + `NativeCallable` 分派模式已存在 | zlib / child_process 需要同样桥接 |
| CLI | `run` 执行文件，`test` 执行测试文件；无 `install` | package scripts / PATH 注入 / 基础 registry install 缺失 |

Baseline Role Alignment：**aligned（scope: both）**。需求层要提高 npm 生态可用性；架构层应沿用“JS builtin 负责 Node API 形状，Rust host bridge 负责宿主 I/O/压缩/进程”边界，不把 Node API 形状散落到 backend 或 semantic。

## 2. 目标与非目标

### 目标

1. 新增 Node.js `stream` builtin：`Readable`、`Writable`、`Duplex`、`Transform`、`PassThrough`、`pipeline`、`finished`、`Readable.from`、`Readable.toWeb/fromWeb`、`Writable.toWeb/fromWeb`、核心事件、对象模式和 highWaterMark/drain 背压信号。
2. 新增 `http` / `https` builtin 的客户端 API：`request()`、`get()`、`ClientRequest`、`IncomingMessage`，基于现有 `fetch` 与新 `stream` 适配。
3. 新增 `zlib` builtin：同步 gzip/gunzip/deflate/inflate/raw/brotli API，以及基于 `Transform` 的 create* 流工厂。
4. 新增 `child_process` builtin：`spawnSync`、`execSync`、`spawn`、`exec`、`ChildProcess` 事件对象、stdout/stderr 捕获流、sandbox allowlist。
5. 新增 CLI package scripts：`wjsm run <script>`、`wjsm test` 优先运行 `package.json` scripts，并为 scripts 注入 `node_modules/.bin` 与 wjsm 二进制所在目录到 PATH。
6. 新增基础 `wjsm install <pkg>`：从 npm registry 下载 tarball，解压到 `node_modules` 布局，并写入 `package.json.dependencies`。
7. 新增 fixtures 覆盖 ESM/CJS builtin 加载和关键可观察行为。

### 非目标

- `http.Server` / `https.Server` 真监听 TCP：issue #311 自身标注依赖 `net`/TCP host import（#313），本设计不伪造不可运行 server；`createServer`/`Server.listen` 返回明确错误。
- HTTP/2、HTTP/3、cluster、inspector、N-API、net/tls/dgram、worker_threads、dns、vm。
- 完整 npm solver / CAS / lockfile / workspace：这些由 package-management 设计承接；本 issue 的 `install` 是 node_modules 兼容基础路径。
- child_process 的真异步管道写入与实时 stdout/stderr streaming；异步 `exec/spawn` 以同步 host 执行结果调度事件，直到 runtime 引入进程 side-table 与 async waiter。

## 3. Architecture Integrity Lens

- Invariant：Node API 外形由 `crates/wjsm-module/builtin_js/node_*.js` 拥有；Rust runtime 只暴露必要 host method。
- Canonical owner / contract：`builtin_modules.rs` 注册 canonical builtin；`runtime_node_globals.rs` 安装 `__wjsm_node_zlib` / `__wjsm_node_child_process`；`NativeCallable::{ZlibMethod, ChildProcessMethod}` 进入统一 host 分派。
- Responsibility overlap：不复用 WHATWG Streams 实现来假装 Node stream；两者只通过 `toWeb/fromWeb` 做互操作。
- Higher-level simplification：新增 `runtime_node_data` 共享 Buffer/string 字节提取，避免 crypto/zlib 重复编码解析。
- Retirement / falsifier：`require('stream')`、`import 'node:zlib'`、`http.get()`、`spawnSync()`、`wjsm run <script>` 均由 fixtures 证明可用；`http.Server` 必须明确指向 #313，不留下静默 no-op。
- Verdict：proceed，采用 JS builtin + Rust host bridge + CLI scripts/install owner 文件。

## 4. 方案对比

| 方案 | 内容 | 优点 | 风险 | 结论 |
|---|---|---|---|---|
| A. 全部 Rust host 内置 | 在 runtime 直接建 Node stream/http 对象 | host 侧控制强 | Node API 形状散落 Rust，维护成本高，难复用 EventEmitter | 拒绝 |
| B. 全部 JS polyfill | stream/http/child_process/zlib 全用 JS | API 外形清晰 | zlib/child_process 无法纯 JS 正确实现宿主能力 | 不足 |
| C. JS builtin + Rust host bridge | JS 管 API，Rust 只做压缩/子进程等宿主边界 | 符合现有 fs/crypto 模式，测试面清晰 | 需要同步 snapshot NativeCallable ABI | **推荐** |

## 5. 兼容边界

- `node:` 前缀和裸 specifier 均解析到同一个 canonical builtin。
- CJS `require()` 继续通过 runtime module loader 返回 builtin default export；ESM import 返回 namespace。
- 新增 `NativeCallable` 为无状态分派项，必须同步更新 `SnapshotNativeCallable`、`abi_hash()` discriminant 范围和 startup bridge。
- child_process 默认拒绝执行；`WJSM_CHILD_PROCESS_ALLOW=*` 或逗号/路径分隔 allowlist 才允许命令。这是 issue 安全要求的一部分。
- `wjsm install` 写 `package.json` 和 `node_modules`，不触碰 future CAS store。

## 6. 验证策略

- `cargo nextest run -E 'test(modules__node_builtin_stream) | test(modules__node_builtin_http) | test(modules__node_builtin_zlib) | test(modules__node_builtin_child_process)'`
- `cargo nextest run -p wjsm-runtime -E 'test(zlib) | test(child_process) | test(snapshot)'`
- `cargo nextest run -p wjsm-cli -E 'test(package_script) | test(install)'`
- CLI smoke：`cargo run -- run -e "const {PassThrough}=require('stream'); const p=new PassThrough(); p.on('data', x=>console.log(x.toString())); p.write('ok');"`

## 7. Working artifacts

TaskIntentDraft：
- Outcome：issue #311 的依赖无关 Node builtin 与 CLI tooling 可执行。
- Success evidence：目标 fixtures 与 crate tests 通过。
- Stop condition：全部目标模块可 import/require，关键 API 输出与 Node 兼容；#313 依赖项输出明确错误。
- Non-goals：TCP server、完整包管理器、实时 child_process stream。
- Risks：snapshot ABI、child_process 安全边界、http wrapper 依赖 fetch promise 行为。

BaselineReadSetHint：
- issue #311、`AGENTS.md`、现有 builtin JS、`runtime_node_fs.rs` / `runtime_node_crypto.rs`、snapshot native bridge、CLI command dispatch。

BaselineUsageDraft：
- Required baseline refs：issue #311、AGENTS、runtime module loading spec、package-management spec、现有 host bridge。
- Acknowledged before plan refs：已读取 issue、AGENTS 注入上下文、相关 specs 与代码探索结果。
- Cited in design refs：本设计 §1-§6。
- Missing refs：无阻塞；#313 为明确外部依赖。
- Decision：continue。

ImpactStatementDraft：
- Affected layers：`wjsm-module` builtin registry、`wjsm-runtime` host globals/native callables/snapshot ABI、`wjsm-cli` commands、fixtures。
- Owners：JS API owner = builtin JS；host I/O owner = runtime_node_*；CLI owner = new cli modules。
- Invariants：runtime 不拥有 Node API 形状；snapshot ABI 同步；child_process 默认 sandbox。
- Compatibility：现有 fs/crypto/path/events fixtures 不变。

Product Risk Lens：
- Value：提高常见 npm 包可用性。
- Non-goals：不宣称完整 TCP/http server 或完整 package manager。
- Trade-offs：先提供依赖无关 client/sync 能力；server/真 async 依赖后续 substrate。
- Decision needed：用户已免审批，本设计直接进入计划和实现。
