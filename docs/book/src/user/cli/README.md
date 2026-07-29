# 命令行

`wjsm` 是单一可执行文件，所有能力通过子命令暴露。本部分逐个说明子命令的参数、输出和退出行为。

命令按用途分为五组：

| 组 | 子命令 | 用途 |
| --- | --- | --- |
| 执行 | `run`、`eval`、`repl`、`test` | 编译并执行代码 |
| 编译 | `build` | 产出 `.wasm` 文件 |
| 源码检查 | `check`、`lint`、`fmt` | 只读诊断与格式化 |
| 产物分析 | `validate`、`size`、`disasm` | 检查已生成的 `.wasm` |
| 流水线观察 | `dump-ast`、`dump-ir`、`dump-wat` | 导出中间结果 |
| 环境管理 | `init`、`install`、`cache`、`completions`、`version` | 项目与工具链维护 |

`wjsm --help` 列出全部子命令，`wjsm <command> --help` 给出该子命令的完整参数。手册中的参数说明与这两处输出保持一致；出现分歧时以 `--help` 为准。

除 `--help` / `--version` 外，[全局选项](global-options.md)可以跟在任意子命令上，例如 `wjsm run app.js --gc zgc` 与 `wjsm --gc zgc run app.js` 等价。
