# wjsm

> Experimental AOT JavaScript/TypeScript runtime targeting WebAssembly.

wjsm 是一个实验性的 JavaScript/TypeScript 运行时：使用 SWC 解析源码，经过自有语义 IR 将程序提前编译为 WebAssembly，再由 Wasmtime 宿主执行。整个执行链不依赖 V8。

> [!WARNING]
> wjsm 当前版本为 `0.1.0`，尚未完整实现 ECMAScript、Web API 或 Node.js 兼容层。它适合参与运行时开发、验证 AOT/Wasm 方案和运行已覆盖的程序；请不要默认任意 npm 包或现有 Node.js 应用都能直接运行。

## 当前能力

| 领域 | 当前状态 |
| --- | --- |
| 执行后端 | Wasm + Wasmtime 是当前可用后端；JIT 只有接入契约，`--target jit` 尚未实现 |
| 源码 | 按扩展名解析 `.js`、`.mjs`、`.cjs`、`.jsx`、`.ts`、`.tsx`；TypeScript 类型语法会参与解析和降级，但 wjsm 不是类型检查器 |
| 语言语义 | 已覆盖作用域与 TDZ、闭包、类、异常、生成器、`async`/`await`、Promise、集合、TypedArray、Proxy/Reflect 等大量语义；完整度以测试和 Test262 结果为准 |
| 模块与包 | 支持 ESM、CommonJS、动态加载、`node_modules` 解析、条件导出和内置 `install` 命令；Node.js 与 npm 生态兼容性仍是子集 |
| Web/Node API | 已实现 Fetch、Streams、定时器、`async_hooks`、`worker_threads`、`vm`、`perf_hooks` 等已覆盖能力；不是完整 Node.js 替代品 |
| 内存管理 | 统一 ManagedHeap，可选择 `mark-sweep`、`g1` 或 `zgc`；启动快照默认启用 |
| 工具链 | 提供运行、构建、检查、lint、格式化、测试、REPL、IR/AST/WAT 输出、Wasm 验证与反汇编、缓存和 shell 补全 |

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

## AOT 构建

```bash
./target/release/wjsm build app.ts -o app.wasm
./target/release/wjsm validate app.wasm
./target/release/wjsm size app.wasm
```

生成的 Wasm 模块使用 wjsm 的宿主 ABI 和 support module，不是可直接交给任意 WASI/WebAssembly 运行时的独立程序。日常执行请使用 `wjsm run`；嵌入场景应通过 `wjsm-host-wasm` 提供宿主能力。

## 常用命令

| 命令 | 用途 |
| --- | --- |
| `wjsm run <file>` | 编译并运行 JS/TS，可用 `--watch` 监听文件 |
| `wjsm build <file> -o <file.wasm>` | 生成 Wasm，`--stage` 可停在 `parse`、`lower`、`compile` 或 `execute` |
| `wjsm check <file>` | 解析并检查错误，不执行程序 |
| `wjsm test <path>` | 运行 `*.test.js`、`*.test.ts`、`*_test.js`、`*_test.ts` |
| `wjsm lint <file>` / `wjsm fmt <file>` | 检查或格式化源码；`fmt -w` 写回文件 |
| `wjsm eval '<expr>'` / `wjsm repl` | 计算表达式或进入交互式 REPL |
| `wjsm init <dir>` / `wjsm install [packages...]` | 创建项目或安装 npm 包 |
| `wjsm dump-ast` / `dump-ir` / `dump-wat` | 查看编译流水线中间结果 |
| `wjsm validate` / `size` / `disasm` | 检查和分析 Wasm 产物 |
| `wjsm cache stats` / `cache clear` | 查看或清理编译缓存 |
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
wjsm-semantic (scope analysis + IR lowering)
      │
      ├── wjsm-module (ESM / CJS graph and resolution)
      ▼
wjsm-backend-wasm (IR → WebAssembly)
      │
      ▼
wjsm-host-wasm (Wasmtime + host APIs + ManagedHeap)
```

ECMAScript/Web/Node 语义算法位于后端无关的 `wjsm-builtins`，宿主契约位于 `wjsm-host`，GC 与对象堆抽象位于 `wjsm-gc`。`wjsm-runtime` 保留为兼容 facade。当前生产路径只有 Wasm 后端，但仓库已经定义新后端的静态接入契约。

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

- [架构决策记录](./docs/adr/)：运行时、GC、快照、模块、调试器与多后端边界的权威设计记录。
- [新后端实现指南](./docs/backend-implementation-guide.md)：实现 `HeapMemory`、`ExecContext` 和 `JsBackend` 的完整接入路径。

## 许可证

仓库目前尚未提供 `LICENSE` 文件。除非项目维护者另行授权，请不要假定代码已按某个开源许可证发布。
