# 用户手册

这本手册讲怎么把 wjsm 装上、跑起来、让它干活——安装、运行代码、编译成 WebAssembly、调配置、查命令参数、定位报错。

wjsm 把 JavaScript/TypeScript 提前编译成 WebAssembly，再由内置的 Wasmtime 宿主执行，整条链路里没有 V8 参与。当前版本 `0.1.0`，ECMAScript 和 Node.js 兼容层都还只是子集——具体支持到什么程度以 `fixtures/` 目录里的 1300+ 行为用例和 Test262 的实际通过情况为准。

## 怎么用这本手册

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

## 几个贯穿全书的约定

- 命令示例统一写成 `wjsm ...`。如果你还没把二进制放进 `PATH`，把它换成 `./target/debug/wjsm` 或 `cargo run --` 就行。
- 标注「未实现」的能力不要在生产路径上依赖——比如 `--target jit` 传了会直接报错退出。
- 章末的「深入了解」链到[内部手册](../internals/index.html)，讲的是实现机制。用 wjsm 不需要读那一半；想了解「为什么这样设计」再去翻。
