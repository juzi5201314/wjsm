# wjsm

> Experimental AOT JavaScript/TypeScript runtime targeting native x86_64 Linux and Windows.

wjsm 是一个实验性的 JavaScript/TypeScript 运行时：使用 SWC 解析源码，经过自有语义 IR，生成可跨平台携带的 portable `.wjsm` 制品，再由 Cranelift 编译为当前宿主的 native image 并由 NativeRuntime 执行。整个执行链不依赖 V8。

> [!WARNING]
> wjsm 当前版本为 `0.1.0`，尚未完整实现 ECMAScript、Web API 或 Node.js 兼容层。它适合参与运行时开发、验证 portable AOT/native runtime 方案和运行已覆盖的程序；请不要默认任意 npm 包或现有 Node.js 应用都能直接运行。

## 当前能力

| 领域 | 当前状态 |
| --- | --- |
| 执行后端 | Direct Cranelift native backend；portable `.wjsm` 是跨平台 semantic-IR 制品，native image 仅进入当前宿主缓存 |
| 源码 | 按扩展名解析 `.js`、`.mjs`、`.cjs`、`.jsx`、`.ts`、`.tsx`；TypeScript 类型语法会参与解析和降级，但 wjsm 不是类型检查器 |
| 语言语义 | 已覆盖作用域与 TDZ、闭包、类、异常、生成器、`async`/`await`、Promise、集合、TypedArray、Proxy/Reflect 等大量语义；完整度以测试和 Test262 结果为准 |
| 模块与包 | 支持 ESM、CommonJS、动态加载、`node_modules` 解析、条件导出和内置 `install` 命令；Node.js 与 npm 生态兼容性仍是子集 |
| Web/Node API | 已实现 Fetch、Streams、定时器、`async_hooks`、`worker_threads`、`vm`、`perf_hooks` 等已覆盖能力；不是完整 Node.js 替代品 |
| 内存管理 | 统一 ManagedHeap，可选择 `mark-sweep`、`g1` 或 `zgc`；启动快照默认启用 |
| 工具链 | 提供运行、构建、检查、lint、格式化、测试、REPL、IR/AST/CLIF 输出、native 诊断、缓存和 shell 补全 |

## 快速开始

需要 Git、Cargo，以及支持 Rust 2024 Edition 的稳定版 Rust 工具链。普通构建不需要拉取 Test262 子模块。

```bash
git clone https://github.com/juzi5201314/wjsm.git
cd wjsm
cargo build --release
./target/release/wjsm version --extended
```

直接运行一段 TypeScript：

```bash
./target/release/wjsm run -e 'const message: string = "Hello, wjsm"; console.log(`${message}: ${1 + 2}`)'
```

输出：

```text
Hello, wjsm: 3
```

也可以运行文件或只计算一个表达式：

```bash
./target/release/wjsm run app.ts
./target/release/wjsm eval '1 + 2 * 3'
```

## Portable AOT 构建

```bash
./target/release/wjsm build app.ts -o app.wjsm
./target/release/wjsm validate app.wjsm
./target/release/wjsm size app.wjsm
./target/release/wjsm run app.wjsm
```

`app.wjsm` 只包含经过验证的 semantic IR、模块清单及可选 source metadata，能够跨平台携带；Cranelift object、relocation、可执行 image 和 native cache 都是当前宿主私有派生数据。`--format native-executable` 当前返回稳定的未实现错误，不创建或覆盖输出文件。

## 常用命令

| 命令 | 用途 |
| --- | --- |
| `wjsm build <file> -o <file.wjsm>` | 生成 portable semantic-IR 制品，`--stage` 可停在 `parse`、`lower`、`compile` 或 `execute` |
| `wjsm check <file>` | 解析并检查错误，不执行程序 |
| `wjsm test <path>` | 运行 `*.test.js`、`*.test.ts`、`*_test.js`、`*_test.ts` |
| `wjsm lint <file>` / `wjsm fmt <file>` | 检查或格式化源码；`fmt -w` 写回文件 |
| `wjsm eval '<expr>'` / `wjsm repl` | 计算表达式或进入交互式 REPL |
| `wjsm init <dir>` / `wjsm install [packages...]` | 创建项目或安装 npm 包 |
| `wjsm dump-ast` / `dump-ir` / `dump-clif` | 查看编译流水线中的 AST、semantic IR 或 Cranelift IR |
| `wjsm validate` / `size` / `disasm` | 校验 portable 制品，或分析当前宿主的 native image |
| `wjsm cache stats` / `clear` / `prune --max-bytes N` | 查看、清空或按旧到新修剪编译缓存 |
| `wjsm completions <shell>` | 生成 shell 补全脚本 |

所有接收源码的主要命令都支持 `-e <SOURCE>`；文件参数使用 `-` 时从标准输入读取。完整参数以 `wjsm <command> --help` 为准。

## 运行时选项

全局选项可以与 `run`、`build` 等子命令组合：

```bash
wjsm --gc zgc --max-heap-size 512M run app.ts
wjsm --inspect=127.0.0.1:9229 run app.ts
wjsm --browser --condition development run app.ts
```

- `--gc <mark-sweep|g1|zgc>`：选择垃圾回收器。
- `--max-heap-size <SIZE>`：限制 JavaScript 堆，支持 `K`、`M`、`G` 后缀。
- `--inspect[=<HOST:PORT>]` / `--inspect-brk[=<HOST:PORT>]`：启用 Chrome DevTools Protocol inspector；当前只提供运行时已实现的调试能力，不等同于完整 Node.js inspector。
- `--browser` / `--condition <NAME>`：控制包解析条件。
- `--config <PATH>`：从 `wjsm.toml` 或 `wjsm.json` 读取默认配置。
- `--time` / `--stats` / `--verify-ir`：输出流水线耗时、统计或校验 IR。

## 架构概览

```text
JS / TS source
      │
      ▼
wjsm-parser (SWC AST)
      │
      ▼
wjsm-semantic (scope analysis + verified IR lowering)
      ├── wjsm-module (ESM / CJS graph and resolution)
      ▼
wjsm-artifact-format (portable .wjsm)
      │
      ▼
wjsm-backend-native (IR → Cranelift CLIF → relocatable native object)
      │
      ▼
wjsm-host-native (native image/cache + ManagedHeap + host APIs)
```

ECMAScript/Web/Node 语义算法位于后端无关的 `wjsm-builtins`，宿主契约位于 `wjsm-host`，GC 与对象堆抽象位于 `wjsm-gc`。`wjsm-runtime` 保留为 native runtime 的公共 facade。当前 production capability 只承诺 x86_64 Linux 与 x86_64 Windows；其他 target 在 native compiler 初始化时返回结构化 capability error，不切换到另一执行后端。

Direct native code 不提供 Wasm memory/control-flow sandbox。artifact verifier、checked lowering、strict relocation、symbol allowlist 与 W^X 属于受信编译/加载边界，不等同于同进程隔离；运行不受信任代码时必须使用独立 OS process、权限隔离与资源限制。

## 开发与测试

项目使用 Rust 2024 和 Cargo workspace。测试优先使用 [cargo-nextest](https://nexte.st/)：

```bash
cargo build
cargo nextest run --workspace
cargo nextest run -E 'test(happy__)'
```

Test262 仅在运行一致性测试时需要：

```bash
git submodule update --init test262
cargo run --release -p wjsm-test262 -- run --suite test/built-ins --plain
```

本仓库不会用静态 Roadmap 表宣称完整兼容性。可观察行为由 `fixtures/`、crate 测试和 Test262 runner 持续验证；尚未覆盖的语义应视为不受支持。

## 深入阅读

- [架构决策记录](./docs/adr/)：运行时、GC、快照、模块、调试器与 Direct Cranelift 终态的权威决策记录。
- [Direct Cranelift 后端实现指南](./docs/backend-implementation-guide.md)：portable artifact、CLIF/image/cache、NativeRuntime 与 ManagedHeap 的维护契约。

## 许可证

仓库目前尚未提供 `LICENSE` 文件。除非项目维护者另行授权，请不要假定代码已按某个开源许可证发布。
