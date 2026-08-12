# 控制流代码生成

IR 的 CFG 在 direct native 后端被编译为 Cranelift 的结构化控制流。这一章说明映射规则。

## CFG → CLIF block

IR 的 `BasicBlock` 直接映射为 CLIF block。终结器决定 block 间的跳转：

| IR 终结器 | CLIF 映射 |
| --- | --- |
| `Return { value }` | `return` 指令 |
| `Jump { target }` | `jump` 到目标 block |
| `Branch { cond, true, false }` | `brif`，条件跳转 |
| `Switch { value, cases, default, exit }` | `br_table` |
| `Throw { value }` | 通过返回值协议传播，不使用 trap |
| `Unreachable` | `trap`（仅用于校验失败路径，production code 不应到达） |

## Phi → block parameters

IR 的 `Phi { dest, sources }` 降级为 CLIF block parameters。每个前驱块在跳转前将对应值写入目标 block 的参数槽。Cranelift 自动处理 phi 的 block parameter 映射。

这与 WASM 结构化控制流不同——CLIF 是非结构化的，不需要 `block`/`loop`/`if` 嵌套。

## 循环

IR 没有专门的循环结构。循环通过 `Jump` 回到前驱 block 表示。CLIF 不区分前向跳转和后向跳转，Cranelift 自行做循环分析和优化（LICM 等）。

wjsm IR 层的 `inline_for_ea` pass 在 lowering 后做跨函数内联和逃逸分析，但循环不变量提升由 Cranelift 的 egraph/LICM 负责。

## 异常传播

异常是 NaN-box 值，标签为 `TAG_EXCEPTION`。可能抛出的操作返回该标签值时，调用方检查并向上传播。不需要 CLIF 的异常处理指令。

每个可能抛出的调用点生成一个检查分支：

```text
%result = call host_op(...)
%is_exc = call is_exception(%result)
brif %is_exc, exception_block, continue_block
```

## 深入了解

- [基本块与控制流图](../ir/cfg.md)
- [控制流与异常](../frontend/control-flow-and-exceptions.md)
- [编译器内部结构](compiler-architecture.md)
