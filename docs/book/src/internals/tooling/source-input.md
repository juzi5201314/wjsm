# 源码输入与编译编排

这一章说明 CLI 如何处理源码输入和编排编译。

## 输入模式

`cli_scripts.rs` 处理三种源码输入：

| 模式 | 触发 | 行为 |
| --- | --- | --- |
| 文件路径 | `wjsm run app.js` | 读文件，作为入口 |
| 内联源码 | `wjsm run -e '...'` | `-e` 参数作为源码 |
| 标准输入 | `cat foo.js \| wjsm run` | 从 stdin 读源码 |

文件路径优先于 `-e`，`-e` 优先于 stdin。

## 编译编排

`lib.rs` 的 `run` 函数编排编译：

1. 解析 CLI 参数，合并配置。
2. 读取源码输入。
3. 调用 `wjsm-host-wasm` 的 `compile_source` 编译为 WASM。
4. 调用 `execute_with_options` 执行 WASM。

不同 subcommand 在步骤 3/4 之间有差异：`build` 只编译不执行，`check` 只到 semantic IR，`dump-*` 在不同阶段输出。

## 预编译入口

`PrecompiledEntry` 是预编译 handoff 结构，记录源码路径和 WASM 路径。`--precompiled` 选项让同入口 fork 时子进程直接加载 raw WASM，跳过再编译。详见[隐藏命令与预编译执行](precompiled-execution.md)。

## 多文件入口

`build` 和 `run` 支持多文件入口。`wjsm-module` 解析入口文件的 import/export，构建模块图，bundle 成单个 IR Program。详见[模块图与 Bundling](../modules/program-bundling.md)。

## 深入了解

- [CLI 参数模型与配置合并](cli-and-config.md)
- [隐藏命令与预编译执行](precompiled-execution.md)
- [编译编排入口](../pipeline/orchestration.md)
