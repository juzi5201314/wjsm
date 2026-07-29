# 命令行配置项

这一章按用途归类全局选项，说明哪些能写进配置文件、哪些只能在命令行给出。逐项语法见[全局选项与通用规则](../cli/global-options.md)。

## 可写入配置文件的选项

`quiet`、`verbose`、`time`、`stats`、`verify-ir`、`color`、`no-color`、`target`、`browser`、`condition`、`max-heap-size`，加上子命令级的 `root` 与 `script`。

## 只能在命令行或环境变量给出的选项

| 选项 | 对应环境变量 |
| --- | --- |
| `--gc <GC>` | `WJSM_GC`、`WJSM_TEST_GC` |
| `--shadow-stack-max <SIZE>` | `WJSM_SHADOW_STACK_MAX` |
| `--wasmtime-memory-reservation <SIZE>` | 无 |
| `--inspect` / `--inspect-brk` | 无 |
| `--config <PATH>` | 无 |

`--config` 本身不可能来自配置文件，其余四项当前没有配置文件键，写进 `wjsm.toml` 会被忽略。

## 按用途归类

诊断输出：`-q`、`-v`、`--time`、`--stats`、`--verify-ir`、`--color` / `--no-color`。

模块解析：`--browser`、`--condition`、以及子命令的 `--root`。

内存与执行：`--max-heap-size`、`--shadow-stack-max`、`--wasmtime-memory-reservation`、`--gc`。

调试：`--inspect[=HOST:PORT]`、`--inspect-brk[=HOST:PORT]`。

后端：`--target`，当前只有 `wasm` 可用。

## 深入了解

- [CLI 参数模型与配置合并的实现](../../internals/tooling/cli-and-config.md)
- [Engine 配置如何把这些选项翻译成 Wasmtime 设置](../../internals/host-runtime/engine-configuration.md)
