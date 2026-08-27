# CLI 参数模型与配置合并

这一章说明 `wjsm-cli` 如何解析命令行参数和合并配置。

## 参数模型

`crates/wjsm-cli/src/cli_args.rs` 定义 CLI 参数模型。顶层命令枚举包含 `run`、`build`、`test`、`check`、`lint`、`eval`、`repl`、`fmt`、`install`、`cache`、`completions`、`init`、`version`、`dump-ast`、`dump-ir`、`dump-clif`、`validate`、`size`、`disasm` 等 subcommand。

每个 subcommand 有自己的参数集，通过 `clap` derive 宏定义。全局选项（`--inspect`、`--color` 等）在顶层 `Cli` 结构上。

verbose 编译提示是 `Compiling portable artifact...`。`--stats` 打印 IR 计数（Constants / Functions / Basic Blocks / Instructions）；存在 portable artifact 时追加字节数；执行路径再打印 `Native cache: entries, bytes, hits, misses, invalidated`。

## `--format native-executable`

`wjsm build --format native-executable` 把 portable `.wjsm` 打进同宿主 ELF/PE：预链 `wjsm-exec` stub + zstd overlay + 制品内源码快照（ADR 0016–0019）。产物不可移植。`WJSM_EXEC_STUB` 可覆盖 stub 路径。打包失败 fail-closed：不创建、不覆盖 `-o` 目标。

## 配置合并

`cli_config.rs` 负责合并配置来源，优先级从高到低：

1. CLI 参数（`--inspect` 等）
2. 环境变量（`WJSM_CACHE_DIR`、`WJSM_OPT_LEVEL` 等）
3. 项目配置文件（`wjsm.toml` / `wjsm.json`）
4. 默认值

合并后的配置交给 `wjsm-host-native` 的 `NativeRuntimeConfig`。没有 `WJSM_COMPILER`。

## 退出码

CLI 退出码遵循固定约定：0 成功，1 编译错误，2 运行时错误，3 用法错误。`process_exit_code` 返回当前执行的退出码。

## 深入了解

- [源码输入与编译编排](source-input.md)
- [运行时配置与环境变量索引](../reference/runtime-configuration-index.md)
- [用户侧的全局选项与通用规则](../../user/cli/global-options.md)
- [用户侧的配置来源与优先级](../../user/configuration/sources-and-precedence.md)
