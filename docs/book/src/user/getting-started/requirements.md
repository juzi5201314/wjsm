# 系统要求

wjsm 目前只以源码形式分发，需要自己构建。这一章列出构建和运行分别需要什么。

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

网络能力（`fetch`、TLS）需要机器可以出网，但这只在程序真的发起请求时才用到。

## 可选组件

| 组件 | 何时需要 | 获取方式 |
| --- | --- | --- |
| `cargo-nextest` | 跑项目测试套件 | `cargo install cargo-nextest` |
| `test262` 子模块 | 跑 ECMAScript 一致性测试 | `git submodule update --init test262` |
| `mdbook` | 本地构建这份手册 | `cargo install mdbook` |

常规构建和使用都不需要 Test262 子模块。

## 平台

开发与验证主要在 Linux 上进行（包括 WSL2）。`process.platform` 与 `process.arch` 按实际编译目标报告。涉及 Linux 特有机制的能力（子进程地址空间限制等）在其他平台上的行为未经系统验证。

## 深入了解

- [Cargo Workspace 与依赖图](../../internals/build-release/workspace-and-dependencies.md)
- [Cargo Feature 与 Profile 的划分](../../internals/build-release/features-and-profiles.md)
