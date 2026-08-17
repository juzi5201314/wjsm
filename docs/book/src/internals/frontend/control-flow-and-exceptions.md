# 控制流与异常

这一章说明 lowering 如何把 JS 控制流构造成基本块图，以及 abrupt completion 的清理序列如何生成。

## 终结器种类

`wjsm_ir::Terminator` 只有六种：

| 终结器 | 用途 |
| --- | --- |
| `Return { value }` | 函数返回，`value` 可缺省 |
| `Jump { target }` | 无条件跳转 |
| `Branch { condition, true_block, false_block }` | 二路分支 |
| `Switch { value, cases, default_block, exit_block }` | `switch` 语句，`cases` 为常量到块的映射 |
| `Throw { value }` | 抛出 |
| `Unreachable` | 不可达路径 |

循环、`if`、逻辑短路、可选链全部由 `Branch` 加 `Jump` 组合而成，IR 层没有专门的循环结构。

> <details><summary>为什么 IR 不直接表达循环？</summary>
>
> 高级 IR（rustc MIR、Swift SIL）通常有专门的循环结构，便于数据流分析和循环不变量外提。wjsm IR 没有：循环用「Jump 到自己的前驱块」表示。
>
> 当前下游是 Cranelift CLIF。CLIF 是非结构化 CFG（`jump` / `brif` / `br_table`），不要求 IR 先还原成 `block`/`loop`/`if` 嵌套。历史上 WASM 后端曾把同一张 CFG 压成结构化控制流；那条路径已删除。
>
> IR 数据结构因此更简单，lowering 也不必为每种循环维护不同的结构。循环不变量提升由 Cranelift 负责，不在 IR 层再做一套。
>
> </details>

## Abrupt completion 的展开

`break`、`continue`、`return` 跨越 `try-finally` 或 `for-of` 时，必须按 ECMAScript 语义先执行清理。`lowerer_branching.rs` 的 `emit_unwind_for_abrupt` 承担这件事：

1. 从内层向外层遍历待清理的嵌套层。
2. 遇到 `for-of` 迭代器层，按 ES §7.4.6 发射 `IteratorClose`。
3. 遇到 `try-finally` 层，内联 finally 块。
4. `finally` 内部自己产生的 abrupt completion 只继续展开更外层的 finalizer，不重复执行当前层。

`completion` 参数携带 `IteratorClose` 的完成值：正常关闭传 `undefined`，abrupt 时传实际完成值。这个参数在展开过程中会被逐层更新。

## try / catch / finally

`lower_try` 在降级 `try` 语句前保存当前完成状态，把 `catch` 与 `finally` 注册进展开栈，使块内的 `break` / `continue` / `return` 能找到正确的清理层次。异常路径通过带 `TAG_EXCEPTION` 的返回值在调用链上传播，不走 trap，也不走 `exnref`。

## 异常值表示

异常是一个 NaN-box 值，标签为 `TAG_EXCEPTION`（`0x5`）。任何可能抛出的操作返回该标签值时，调用方检查并向上传播。`wjsm_ir::value::is_exception` 是判定入口，编码入口是 `encode_exception_handle`。

Native ABI 把完成状态编进同一个 `i64` 返回值：正常结果与异常共用 NaN-box 空间。生成代码没有独立的 trap / exception 引用通道，因此「抛出」就是带标签的返回。代价是每个可能抛出的调用点都要有检查分支。

## 深入了解

- [基本块与 CFG 的结构约束](../ir/cfg.md)
- [后端如何把 CFG 编成 CLIF](../backend/control-flow-codegen.md)
- [异常与完成记录在后端的落地方式](../backend/exceptions-and-completions.md)
