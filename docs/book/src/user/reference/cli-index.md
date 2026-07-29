# 命令行索引

全部公开子命令与全局选项。参数细节见每个命令自己的章节，或 `wjsm <command> --help`。

## 子命令

| 命令 | 位置参数 | 专属选项 | 用途 |
| --- | --- | --- | --- |
| [`run`](../cli/run.md) | `[INPUT]`、`-- <ARGS>...` | `--root`、`-w/--watch`、`--script`、`-e` | 编译并执行 |
| [`build`](../cli/build.md) | `[INPUT]` | `-o/--output`、`--stage`、`--root`、`--script`、`-e` | 生成 `.wasm` |
| [`test`](../cli/test.md) | `[INPUT]` | `--root`、`--script`、`-e` | 发现并运行测试文件 |
| [`check`](../cli/check.md) | `[INPUT]` | `--root`、`--script`、`-e` | 只做解析与语义检查 |
| [`lint`](../cli/lint.md) | `[INPUT]` | `--root`、`--script`、`-e` | 内置 lint 规则 |
| [`eval`](../cli/eval.md) | `<CODE>` | 无 | 求值单个表达式并打印 |
| [`repl`](../cli/repl.md) | 无 | `-e`、`--script` | 交互式求值 |
| [`fmt`](../cli/fmt.md) | `<INPUT>` | `-w/--write` | SWC codegen 格式化 |
| [`dump-ast`](../cli/dump-ast.md) | `[INPUT]` | `--root`、`--script`、`-e` | 输出 SWC AST JSON |
| [`dump-ir`](../cli/dump-ir.md) | `[INPUT]` | `--format`、`--func`、`--root`、`--script`、`-e` | 输出语义 IR |
| [`dump-wat`](../cli/dump-wat.md) | `[INPUT]` | `--func`、`--skeleton`、`--root`、`--script`、`-e` | 输出 WAT |
| [`validate`](../cli/validate.md) | `<INPUT>` | 无 | 校验 `.wasm` |
| [`size`](../cli/size.md) | `<INPUT>` | 无 | 分节字节数统计 |
| [`disasm`](../cli/disasm.md) | `<INPUT>` | `--func`、`--skeleton` | 反汇编已有 `.wasm` |
| [`cache`](../cli/cache.md) | `stats` \| `clear` | 无 | 查看或清空编译缓存 |
| [`completions`](../cli/completions.md) | `<SHELL>` | 无 | 生成补全脚本 |
| [`init`](../cli/init.md) | `<PATH>` | `--force` | 创建项目骨架 |
| [`install`](../cli/install.md) | `[PACKAGES]...` | 无 | 安装 npm 包 |
| [`version`](../cli/version.md) | 无 | `--extended` | 版本信息 |

## 全局选项

`--config`、`-q/--quiet`、`-v/--verbose`、`--time`、`--stats`、`--verify-ir`、`--color`、`--no-color`、`--target`、`--browser`、`--condition`、`--max-heap-size`、`--shadow-stack-max`、`--wasmtime-memory-reservation`、`--gc`、`--inspect`、`--inspect-brk`。

语义与默认值见[全局选项与通用规则](../cli/global-options.md)。

## 退出码

| 码 | 含义 |
| --- | --- |
| `0` | 成功 |
| `1` | 编译错误、`validate` 失败、`test` 有失败项 |
| `2` | 未捕获的运行时异常 |
| `3` | 参数用法错误 |
| 其他 | `process.exit(n)` 传入的值 |
