# 系统要求

wjsm 目前只以源码形式分发，需要自己从仓库构建。这一章列出构建和运行分别需要什么。

## 构建期

| 组件 | 要求 | 说明 |
| --- | --- | --- |
| Rust 工具链 | 支持 Rust 2024 Edition 的稳定版 | workspace 与所有 crate 都是 `edition = "2024"` |
| Cargo | 随 Rust 安装 | 构建入口 |
| Git | 任意近期版本 | 克隆仓库；Test262 子模块也用它拉取 |
| C 链接器 | 平台默认即可 | Wasmtime 等依赖需要链接 |

首次构建会编译 Wasmtime（含 Cranelift 与 Winch 两个编译器后端）和 SWC，耗时明显长于后续增量构建。

## 磁盘

- 源码树加 `target/` 的 debug 构建产物体积可观，`wjsm` 单个 debug 二进制约 280 MB（含调试信息）。
- 运行期编译缓存默认落在 `$HOME/.cache/wjsm`，会随使用增长。可以用 `wjsm cache stats` 查看占用，`wjsm cache clear` 清理。

## 运行期

构建出来的 `wjsm` 二进制自带执行所需的一切：Wasmtime 引擎、宿主函数实现、垃圾回收器和构建期固化的启动工件。运行时不需要 Node.js，也不需要另外安装 WebAssembly 运行时。

网络能力（`fetch`、TLS）需要机器可以出网——但这只在程序真的发起请求时才用到，wjsm 自身不联网。

## 可选组件

| 组件 | 何时需要 | 获取方式 |
| --- | --- | --- |
| `cargo-nextest` | 跑项目测试套件 | `cargo install cargo-nextest` |
| `test262` 子模块 | 跑 ECMAScript 一致性测试 | `git submodule update --init test262` |
| `mdbook` | 本地构建这份手册 | `cargo install mdbook` |

常规构建和使用都不需要 Test262 子模块——它只在跑一致性测试时拉取。

> <details><summary>为什么「构建时间长」是 wjsm 的客观属性，不只是配置问题？</summary>
>
> wjsm 的依赖链里有两个大头：Wasmtime（完整的 Cranelift + Winch 编译器后端，以及 wasmtime 自身的运行时）和 SWC（完整的 ECMAScript + TypeScript 解析器）。这两个都是「一次编译到位」型的依赖——首次构建时要把整个依赖图编译一遍。
>
> 具体数字：首次 `cargo build --release` 在常见机器上需要 5-15 分钟（取决于 CPU 核数和内存），产出单个 `wjsm` 二进制。Debug 构建会再慢一些，因为某些关键依赖在 dev profile 下默认未优化，wjsm 的 `Cargo.toml` 对它们强制 `opt-level = 3` 才让 debug 跑得动。
>
> 缓解方法：加 `CARGO_BUILD_JOBS=N` 限制并行度可以减少内存峰值；用 `sccache` 缓存编译产物可以跨项目复用。
>
> </details>

## 平台

开发与验证主要在 Linux 上进行（包括 WSL2）。`process.platform` 与 `process.arch` 按实际编译目标报告。涉及 Linux 特有机制的能力（子进程地址空间限制等）在其他平台上的行为未经系统验证。

## 深入了解

- [Cargo Workspace 与依赖图](../../internals/build-release/workspace-and-dependencies.md)
- [Cargo Feature 与 Profile 的划分](../../internals/build-release/features-and-profiles.md)
