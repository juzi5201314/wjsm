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
> 高级 IR（rustc MIR、Swift SIL）通常有专门的循环结构——便于数据流分析、循环不变量外提。wjsm IR 没有，循环用「Jump 到自己的前驱块」表示。
>
> 原因是 wjsm IR 的下游消费者只有 wasm codegen：codegen 把 IR 的 CFG 还原成 WASM 结构化控制流（`block`/`loop`/`if`），由 codegen 决定什么算循环。后端有充分信息做这个判断——它看 Jump 的方向（前向是 if，后向是 loop）。
>
> 好处是 IR 数据结构更简单、lowering 不用为每种循环维护不同的结构。代价是有些分析（数据流、循环不变量外提）wjsm 没法做——但 wjsm 也不做这些优化，Cranelift/Winch 负责。
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

`lower_try` 在降级 `try` 语句前保存当前完成状态，把 `catch` 与 `finally` 注册进展开栈，使块内的 `break` / `continue` / `return` 能找到正确的清理层次。异常路径本身不依赖 WASM 的异常处理指令，而是通过带 `TAG_EXCEPTION` 的返回值在调用链上传播。

## 异常值表示

异常是一个 NaN-box 值，标签为 `TAG_EXCEPTION`（`0x5`）。任何可能抛出的操作返回该标签值时，调用方检查并向上传播。`wjsm_ir::value::is_exception` 是判定入口，编码入口是 `encode_exception_handle`。

这个设计把「抛出」变成普通返回值检查，代价是每个可能抛出的调用点都要有检查分支，收益是不依赖 WASM 异常提案。

> <details><summary>为什么不用 WASM 异常处理提案？</summary>
>
> WASM 异常处理提案（`exnref`、`try_table` 等）提供原生异常支持，看起来更优雅：异常值是独立的引用类型，`try_table` 直接处理，不用每个调用点手动检查。
>
> 但 wjsm 不用的原因是：
>
> 1. **跨运行时兼容性差**。不同 wasmtime 版本、Wasm Micro Runtime、wazero 对异常处理提案的支持进度不一。wjsm 产物要在多个宿主上能跑。
> 2. **后端复杂度**。codegen 要理解异常引用的表示、catch 子句的注册机制——这又是一层 IR 状态。
> 3. **性能不一定更好**。「每次调用都检查返回值」的开销在多数场景下是常数次的指令，WASM 异常处理也要在调用边界做检查（虽然形式不同）。
>
> 实际结果是：wjsm 产物在通用 wasmtime 下能跑，依赖更少，后端实现更简单。
>
> </details>

## 深入了解

- [基本块与 CFG 的结构约束](../ir/cfg.md)
- [后端如何把 CFG 编码成 WASM 结构化控制流](../backend/control-flow-codegen.md)
- [异常与完成记录在后端的落地方式](../backend/exceptions-and-completions.md)
