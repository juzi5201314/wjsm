# wjsm

wjsm 的产品目标是成为**轻量、现代、低内存占用，但与 Node.js / V8 同性能等级**的 JavaScript/TypeScript 运行时：给开发者 Node 级的吞吐与生态入口（CLI、模块、包解析、常用内置），用更小的进程体量和常驻内存做到。引擎侧的性能与质量对照物是 V8。

执行链：SWC AST → 已验证 semantic IR → 可携带 `.wjsm` → Cranelift 直接生成 native image → `NativeRuntime`。静态可见模块在第一次执行前编成 generic native；语言仍是动态的（`eval`、overlay）。禁止 V8、Wasm、Wasmtime 或第二套执行后端。

## 产品目标（决策时用）

做取舍时默认朝这个方向：

- **轻量**：一条 native 执行链，不引入第二套 VM；宿主、GC、builtins 保持可拆分、可裁剪；禁止为兼容而堆平行实现。
- **现代**：面向当代 JS/TS 与现行 Node 平台契约（ESM、`node:`、Web 标准全局、当前稳定 API）。不把已废弃 API 或历史包袱当成必须复刻的目标。
- **低内存**：常驻 RSS、堆占用、快照与缓存体积是一等约束。热路径表示要为缓存局部性与少分配服务；禁止用「先堆上去再优化」换功能。
- **同性能等级**：典型 CLI / 服务 / 模块加载工作负载以 V8（经 Node 测得的同场景）为对照档位，对齐这一档的吞吐与延迟，而不是微基准作弊。冷启动、缓存命中后的启动、稳态与内存必须一起看。
- **Node 是平台合同，不是源码移植**：用户可见行为以 ECMAScript 与已承诺的 Node/Web API 为准。缺口补在语义与宿主层；不要为了表面相似而放宽 spec。

当前实现仍是子集（见 README）。目标描述的是方向：补能力时优先高杠杆的现代表面，同时压内存与稳态性能，而不是追求百分百 Node 克隆。

## 不可妥协

- Rust 2024、默认 rustfmt、源码注释用中文、编译零 warning。
- ECMAScript 是语义真理。禁止交付残缺语义、跳过边角或错误的 early error。
- 后端边界遵守 ADR 0014：Cranelift / object / 平台依赖只属于 `wjsm-backend-native` 与 `wjsm-host-native`；`wjsm-builtins`、`wjsm-host`、`wjsm-gc`、`wjsm-module` 保持后端无关。
- 测试只验证正确性：确定性、快速、进程内；禁止重负载/压力、依赖随机的断言、真实网络 I/O（TCP/UDP/HTTP/TLS/DNS）、真实子进程/PTY。
- 生成物放 `/tmp`。临时 JS/TS 用 `-e`，不要落盘草稿源文件。
- 修问题修在归属层，删掉过时路径。禁止靠削弱 fixture / snapshot 掩盖失败。
- 测试失败时直接定位并修。不要靠来回切 commit 证明「不是我引入的」——编译极贵。
- 性能优化看真实场景（启动 RSS、稳态吞吐、模块加载），以 V8/Node 同场景为对照，禁止为基准作弊。激进优化若有副作用必须写明。

## 测试

- 测试只验证正确性：确定性、快速、进程内。禁止 GC churn、大循环、大量分配、随机断言、真实网络或真实子进程/PTY。
- 重负载、随机性、真实网络/进程行为放 `crates/*-bench`、`fuzz/`、`bench/` 或手工命令；慢用例只能进 `slow`/`full` profile。默认 `cargo nextest run --workspace` 必须全是快速正确性测试。
- 验证网络/进程协议时，在宿主层提供确定性测试替身 / transport 钩子（如 `WJSM_TEST_*`），测试只断言协议状态机。
- 新测试冷启动超过默认 profile 的 30s 硬门禁，说明用例过重：拆小、换替身、或移出默认套件，不要加 nextest 黑名单。

## 命令

```bash
cargo build
cargo run -- run -e 'console.log(1 + 2)'
cargo run -- build -e 'console.log(1)' -o /tmp/out.wjsm
cargo run -- check -e 'const x = 1'
cargo run -- dump-ir -e 'const x = 1'
cargo run -- dump-clif -e 'const x = 1'
cargo nextest run --workspace
cargo nextest run -E 'test(happy__hello)'
cargo nextest run -p wjsm-semantic
WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(happy__<name>)'
WJSM_UPDATE_SNAPSHOTS=1 cargo nextest run -p wjsm-semantic -- lowering_snapshots
```

## 工作流

- 先判断失败阶段：parse → lower → module graph → codegen → host/runtime。
- 用 `dump-ast`、`dump-ir`、`dump-clif`、`disasm` 对比相邻阶段；这些路径能暴露问题时不要加临时生产日志。
- lowering 变更需要 semantic IR snapshot。可观察行为用 `fixtures/happy` 或 `fixtures/errors` 加 `.expected`。模块行为放 `fixtures/modules`。
- 接受生成的 fixture/snapshot 前先审内容。先跑窄测试，跨 crate 再跑 workspace。
- 语言问题先对照 spec 原文与本仓库实现，再看真实引擎源码，最后才请用户定语义。
- 文件职责单一（目标 ≤500 行）、函数内聚（目标 ≤30 行）；按 semantic / backend / host 拆分，不要另起一套平行约定。

## 事实来源

- 用户可见行为与 CLI：[README.md](README.md) 与 `wjsm --help`。
- 架构边界与不变量：[docs/adr/](docs/adr/)，尤其是 ADR 0010 与 0014。
- Direct native 后端契约：[docs/backend-implementation-guide.md](docs/backend-implementation-guide.md)。
- Fixture 与测试机制：`build.rs`、`tests/`、`fixtures/`、`.config/nextest.toml`。
