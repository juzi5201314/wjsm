# Workspace crate 地图

这一章给出 15 个 workspace 成员各自的职责、公开面和规模，用于判断某个改动应该落在哪个 crate。

## 成员清单

规模数据取自 `crates/*/src` 下 `.rs` 文件统计，用于体现重心分布，不是精确契约。

| Crate | 职责 | 直接依赖要点 | 规模（文件 / 行） |
| --- | --- | --- | --- |
| `wjsm-ir` | IR 数据结构、NaN-boxing 编码、常量、校验 | 无外部依赖 | 6 / 3754 |
| `wjsm-parser` | 按扩展名选择 SWC 语法并产出 AST，格式化诊断 | `swc_core` | 2 / 264 |
| `wjsm-semantic` | 作用域分析、两阶段 lowering、早期错误 | `swc_core`、`wjsm-ir`、`wjsm-parser` | 55 / 23243 |
| `wjsm-module` | 模块图、解析、CJS 转换、bundling | `swc_core`、`wjsm-semantic`、`wjsm-ir` | 15 / 9131 |
| `wjsm-backend-wasm` | IR → WASM 字节，support 模块，host import 表 | `wasm-encoder`、`wjsm-ir` | 44 / 14952 |
| `wjsm-backend-jit` | JIT 后端接入契约，未实现 | `wjsm-ir`、`wjsm-host` | 1 / 36 |
| `wjsm-host` | 后端无关宿主契约：`ExecContext`、`HeapContext`、`JsBackend` | `wjsm-ir` | 15 / 2729 |
| `wjsm-builtins` | ECMAScript / WHATWG 语义算法，泛型于 `ExecContext` | `wjsm-host`、`wjsm-ir` | 60 / 17328 |
| `wjsm-gc` | 堆访问、Handle 表、mark-sweep / G1 / ZGC | `wjsm-ir`、`parking_lot`、`crossbeam-deque` | 52 / 10844 |
| `wjsm-host-wasm` | Wasmtime 后端：执行、host import、ManagedHeap、Node 模块 | `wasmtime`、`wjsm-gc`、`wjsm-builtins` 等 | 185 / 72223 |
| `wjsm-snapshot-format` | 启动快照二进制格式与重定位 | `wjsm-ir` | 2 / 1267 |
| `wjsm-runtime` | 兼容 facade，只 re-export | `wjsm-host`、`wjsm-host-wasm`、`wjsm-gc` | 1 / 25 |
| `wjsm-cli` | 参数模型、配置合并、命令实现 | `clap`、`wjsm-*` | 9 / 4649 |
| `wjsm-test262` | Test262 runner，子进程与并发预算 | `clap`、`rayon`、`comfy-table` | 5 / 1836 |
| `wjsm-gc-bench` | GC 基准 runner 与报告 | `wjsm-runtime`、`sysinfo` | 11 / 724 |

> <details><summary>为什么 `wjsm-host-wasm` 占 70% 代码量？</summary>
>
> 它同时承担四件不同的事：
>
> 1. **Wasmtime 引擎集成**：配置、Engine 池化、模块编译、实例化、Store 管理。
> 2. **host import 注册**：约 500+ 个 `env.*` 函数，加上它们的 Rust 实现薄包装。
> 3. **ManagedHeap 接合**：把 wjsm-gc 的 GC 算法接入 wasmtime 的内存模型。
> 4. **Node 内置模块实现**：24 个 `node:*` 模块的 Rust 实现 + 一些 JS polyfill 的注册。
>
> 任何一个单独抽出来都能做个独立 crate（实际上早期版本考虑过），但目前它们耦合度高（host import 的函数经常调用 Node 模块的代码，Node 模块又调用 GC API），分开会让代码更绕。
>
> 内部按目录拆分（`host_imports/`、`exec_context_impl/`、`runtime_gc/`、`inspector/`、`runtime_node_*.rs`），单文件保持 500 行以下。规模大但能维护。
>
> </details>

## 重心解读

`wjsm-host-wasm` 占全仓约七成代码量，因为它同时承载 Wasmtime 执行引擎、host import 注册、ManagedHeap 接合与 Node 内置模块实现。它内部按域拆分为子目录（`host_imports/`、`exec_context_impl/`、`runtime_gc/`、`inspector/` 等），单文件保持在可维护范围。

`wjsm-runtime` 只有 25 行，是 ADR 0011 拆分后的兼容外观。新代码不应向它添加实现。

## 判断落点

| 改动性质 | Crate |
| --- | --- |
| 新语法或新语义 | `wjsm-semantic`（必要时先扩 `wjsm-ir`） |
| 新 IR 指令 | `wjsm-ir` + `wjsm-semantic` + `wjsm-backend-wasm` |
| 新 ECMAScript 算法 | `wjsm-builtins` |
| 新 host import | `wjsm-host-wasm/src/host_imports/` 薄注册层 |
| 新 Node 内置模块 | `wjsm-module` 元数据 + `wjsm-host-wasm` 支撑 |
| GC 或对象布局 | `wjsm-gc`，必要时同步快照格式 |
| 新命令或参数 | `wjsm-cli` |

## 相关章节

- [跨 crate 所有权与依赖边界](ownership-and-dependencies.md)
- [Crate 与公共 API 索引](../reference/crate-api-index.md)
- [多后端边界](../backend/multi-backend-boundary.md)
