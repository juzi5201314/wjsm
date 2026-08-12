# 命令行

`wjsm` 是单一可执行文件。`wjsm --help` 列出所有子命令，`wjsm <command> --help` 是参数语法的当前事实源。

| 组 | 子命令 | 用途 |
| --- | --- | --- |
| 执行 | `run`、`eval`、`repl`、`test` | 编译并执行代码 |
| 构建 | `build` | 生成 portable `.wjsm` |
| 源码检查 | `check`、`lint`、`fmt` | 解析、语义检查与格式化 |
| 流水线观察 | `dump-ast`、`dump-ir`、`dump-clif` | 导出相邻阶段结果 |
| 制品检查 | `validate`、`size`、`disasm` | 验证 artifact、报告体积、反汇编当前宿主 image |
| 环境管理 | `init`、`install`、`cache`、`completions`、`version` | 项目、依赖与 native cache 管理 |

全局选项可以写在子命令前或后。对参数存在疑问时直接运行对应的 `--help`；本书说明其语义，不复制完整 clap 输出。
