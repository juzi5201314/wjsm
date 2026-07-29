# `build`

把 JS/TS 编译成 `.wasm` 文件，或在流水线中途停下来查看中间结果。

```text
wjsm build [OPTIONS] [INPUT]
```

## 输出位置

`-o/--output` 默认 `out.wasm`。写入默认路径且该文件已存在时会打印一条覆盖警告（`-q` 可静音）：

```text
warning: 'out.wasm' already exists and will be overwritten (use `-o` to choose another path)
```

`-o -` 把二进制写到标准输出。检测到标准输出是终端时会拒绝执行，避免终端被二进制刷屏：

```text
refusing to write binary WASM to a terminal; redirect stdout to a file or use `-o <path>`
```

## `--stage`

| 值 | 行为 | 输出目标 |
| --- | --- | --- |
| `parse` | 解析后打印 AST JSON | 标准输出 |
| `lower` | 降级到 IR 后打印 IR 文本 | 标准输出 |
| `compile` | 编译成 WASM 并写文件（默认） | `-o` 指定的路径 |
| `execute` | 编译、写文件，然后执行 | 文件 + 程序输出 |

`parse` 和 `lower` 的结果走标准输出，因此与 `-o` 冲突：

```text
`-o` / `--output` cannot be used with `--stage parse` or `--stage lower` (output goes to stdout)
```

## 统计与计时

```bash
wjsm build app.ts -o /tmp/app.wasm --stats --time
```

`--stats` 在 `compile` 阶段只打印产物字节数；配合 `run` 或 `--stage execute` 时打印完整统计：

```text
=== Statistics ===
  Constants: 21
  Functions: 1
  Basic Blocks: 3
  Instructions: 46
  WASM Size: 25686 bytes
```

`--time` 打印各阶段耗时，`-v` 会把单位从毫秒切到微秒。

## 产物的定位

生成的模块依赖 wjsm 的宿主 import 和 support 模块，不是能丢给任意 WASI 运行时直接跑的独立程序。要执行它，用 wjsm 自身，或在 Rust 里通过 `wjsm-host-wasm` 提供宿主能力。

## 深入了解

- [WASM Import、Export 与主模块 ABI](../../internals/backend/imports-exports-and-abi.md)
- [Support 模块与辅助函数的职责划分](../../internals/backend/support-module.md)
- [编译阶段如何把 IR 变成 WASM](../../internals/pipeline/compile.md)
