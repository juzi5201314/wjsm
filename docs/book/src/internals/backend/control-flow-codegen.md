# 控制流代码生成

IR 的 CFG 是基本块与六种终止器；WASM 是结构化控制流。这一章说明后端如何把前者还原成后者。

## block / loop / if

`compiler_control/control_structured.rs` 为每类 IR 结构生成对应的 WASM 控制原语：

| IR 终止器 | WASM 结构 |
| --- | --- |
| `Jump` 目标是后继块 | 直接 fallthrough，不生成控制原语 |
| `Jump` 向后 | `loop` + `br` |
| `Branch` | `if` + `else` |
| `Switch` | `block` 套 `br_table` + `block` per case |
| `Return` / `Throw` | 直接 `return` / 调用抛出 builtin |

相邻基本块之间的 fallthrough 是最常见的优化路径：如果 IR 块 `bb0` 的 `Jump` 目标是 `bb1`，且 `bb1` 紧随其后，后端不生成任何控制指令，块尾的 `Jump` 自动落到 `bb1` 的第一条指令。

## 前驱收集

`control_analysis.rs` 的 `collect_predecessors` 在生成前遍历 CFG，记录每个块的前驱。用于：

- 确认 fallthrough 是否合法（前驱是否就是上一块）。
- 决定是否需要 `block` 包裹（被多个前驱引用的块需要独立 `block` 入口）。

## Phi 的处理

`control_locals.rs` 的 `allocate_phi_locals` 把 `Phi` 结果分配到 WASM local。后端在进入目标块前，在每个前驱块尾插入 `local.set` 写入对应值，目标块开头 `local.get` 读取。这把 SSA 的 Phi 节点展开成了显式 local 读写。

## `try` 的展开

IR 没有结构化 try：`Throw` 是普通终止器，异常沿宿主调用栈传播。后端把 `try/catch/finally` 展开为：

- try 体按顺序生成指令。
- 每个 `Throw` 终止器变成对 `env.throw` 的调用。
- catch handler 是一个接收 i64 异常值的块。
- finally 块在每个 `break`/`continue`/`return` 的展开路径上被内联复制。

`emit_unwind_for_abrupt`（语义层）已经算好清理序列，后端只按它给出的层次顺序发射指令，不需要重新推导。

## 深入了解

- [语义层如何构造 CFG 和 abrupt completion 展开](../frontend/control-flow-and-exceptions.md)
- [基本块与终止器形状](../ir/cfg.md)
- [异常值如何在调用链上传播](exceptions-and-completions.md)
