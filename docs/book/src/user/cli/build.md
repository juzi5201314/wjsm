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

> <details><summary>为什么 `wjsm build` 不产出独立可执行文件？</summary>
>
> 这是一个被反复问的问题，简短回答是：因为 wjsm 的设计就是「JS 逻辑」+「wjsm 宿主」两个东西分离。
>
> 长一点的解释：JS 程序用到的东西里，有相当一部分（属性查找、对象分配、字符串处理、Promise 各种方法……）需要跨语言调用——你不能让 WASM 模块自己实现「`Array.prototype.map` 的全部行为」，那是一个完整的 ECMAScript 语义实现。
>
> wjsm 的拆分是：编译产物只关心「JS 怎么写」和「用户代码本身的控制流」，把这些编成 WASM；所有「JS 怎么执行」（属性查找走原型链、对象分配走堆、Promise 链注册 reaction……）放在宿主侧实现。
>
> 这意味着产物离开 wjsm 宿主就跑不起来。如果想要独立 WASM 产物，需要把这套宿主能力也编进 WASM 模块里——技术上可以，但会大幅膨胀产物体积，并且把 WASM 当 CPU 用的好处（沙箱、跨平台）会被「自带一个 JS 引擎」这件事抵消。
>
> </details>

## 深入了解

- [WASM Import、Export 与主模块 ABI](../../internals/backend/imports-exports-and-abi.md)
- [Support 模块与辅助函数的职责划分](../../internals/backend/support-module.md)
- [编译阶段如何把 IR 变成 WASM](../../internals/pipeline/compile.md)
