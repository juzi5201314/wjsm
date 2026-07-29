# 用户手册

这本手册讲怎么用 wjsm：安装、运行代码、编译成 WebAssembly、配置运行时、排查问题。

wjsm 把 JavaScript/TypeScript 提前编译成 WebAssembly，再由内置的 Wasmtime 宿主执行，整条链路不含 V8。当前版本 `0.1.0`，ECMAScript 与 Node.js 兼容层都还是子集，具体覆盖范围以 fixture 与 Test262 结果为准。

## 从哪里开始

| 你的目标 | 去哪一章 |
| --- | --- |
| 先搞清楚 wjsm 是什么、和 Node 有什么不同 | [认识 wjsm](overview/index.html) |
| 装好并跑第一段代码 | [入门](getting-started/index.html) |
| 查某个子命令的参数 | [命令行](cli/index.html) |
| 组织多文件项目、用 npm 包 | [项目、模块与包](projects/index.html) |
| 确认某个语言特性或 Node API 能不能用 | [语言与运行时](runtime/index.html) |
| 调 GC、堆上限、快照、Inspector | [配置](configuration/index.html) |
| 照着可复现的步骤做一件事 | [常用工作流](workflows/index.html) |
| 理解 `.wasm` 产物、退出码、权限边界 | [输出与运行环境](output/index.html) |
| 有报错要定位 | [故障排查](troubleshooting/index.html) |
| 查表 | [用户参考](reference/index.html) |

## 阅读约定

- 命令示例统一写成 `wjsm ...`。如果你还没把二进制放进 `PATH`，把它换成 `./target/debug/wjsm` 或 `cargo run --`。
- 标注「未实现」的能力不要在生产路径上依赖，例如 `--target jit`。
- 章末的「深入了解」指向[内部手册](../internals/index.html)，讲的是实现机制。使用 wjsm 不需要读那一半。
