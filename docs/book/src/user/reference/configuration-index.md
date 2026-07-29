# 配置项索引

`wjsm.toml` / `wjsm.json` 可用的全部键。命令行显式传入的同名选项覆盖文件值。

## 全局键

| 键 | 类型 | 对应命令行选项 |
| --- | --- | --- |
| `quiet` | bool | `-q/--quiet` |
| `verbose` | 整数 | `-v/--verbose` |
| `time` | bool | `--time` |
| `stats` | bool | `--stats` |
| `verify-ir` | bool | `--verify-ir` |
| `color` | `"auto"` \| `"always"` \| `"never"` | `--color` |
| `no-color` | bool | `--no-color` |
| `target` | `"wasm"` \| `"jit"` | `--target` |
| `browser` | bool | `--browser` |
| `condition` | 字符串数组 | `--condition`（可重复） |
| `max-heap-size` | 字节数（整数） | `--max-heap-size` |

## 命令级键

| 键 | 类型 | 作用命令 |
| --- | --- | --- |
| `root` | 路径 | `build`、`run`、`test`、`check`、`lint`、`dump-ir`、`dump-ast`、`dump-wat` |
| `script` | bool | 上述命令，外加 `repl` |

## 不可通过配置文件设置

`--shadow-stack-max`、`--wasmtime-memory-reservation`、`--gc`、`--inspect`、`--inspect-brk`、`--config` 只接受命令行；其中前三项另有环境变量入口。

`max-heap-size` 在文件中必须写成字节整数，`K`/`M`/`G` 后缀只在命令行可用。

完整语义见[配置来源与优先级](../configuration/sources-and-precedence.md)与[`wjsm.toml` 与 `wjsm.json`](../configuration/project-files.md)。
