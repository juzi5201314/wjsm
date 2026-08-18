# 基本块与控制流图

这一章说明 IR 的控制流表示：块怎么组织、有哪几种终止器、CFG 必须满足什么形状。

## BasicBlock

```rust
pub struct BasicBlock {
    id: BasicBlockId,
    instructions: Vec<Instruction>,
    terminator: Terminator,
}
```

每个块是一串无分支指令加**恰好一个**终止器。`BasicBlock::new` 把终止器初始化为 `Terminator::Unreachable`，语义层构造完块体后调 `set_terminator` 覆盖。这个默认值是刻意选的：忘记设置终止器会在 `verify()` 或运行时暴露为 unreachable，而不是静默 fallthrough 到下一个块。

块之间**没有隐式 fallthrough**。要走到下一个块必须显式 `Jump`。

## 六种终止器

| 终止器 | 后继 |
| --- | --- |
| `Return { value: Option<ValueId> }` | 无 |
| `Jump { target }` | 一个 |
| `Branch { condition, true_block, false_block }` | 两个 |
| `Switch { value, cases, default_block, exit_block }` | cases + default |
| `Throw { value }` | 无（异常沿 `TAG_EXCEPTION` 返回值传播） |
| `Unreachable` | 无 |

`Switch` 额外带 `exit_block`。JS 的 switch 有 fallthrough 语义，case 体之间可以顺序流下去，`exit_block` 记录 `break` 的目标，让后端不必从 case 列表反推汇合点。`SwitchCaseTarget { constant, target }` 用 `ConstantId` 而非 `ValueId` 做标签，因为 case 值在 lowering 期已知是常量。

Cranelift 消费的是这张非结构化 CFG：每个 IR 块对应一个 CLIF block，终结器变成 `jump` / `brif` / `br_table` / `return`。历史上 WASM 后端曾把同一张图压成结构化 `block`/`loop`/`if`；当前生产路径不再做这一步。

## Phi

`Instruction::Phi { dest, sources: Vec<PhiSource> }`，`PhiSource { predecessor, value }` 显式记录每个前驱贡献的值。IR 不是严格 SSA——变量通过 `StoreVar`/`LoadVar` 走作用域槽——但控制流汇合处的表达式值用 Phi 合并。

> <details><summary>「Phi 节点」存在的意义</summary>
>
> 严格 SSA 里，每个变量只有一个定义点。控制流汇合处（如 if-else）需要根据从哪条路径来选不同的值。
>
> IR 是「几乎 SSA」：`ValueId` 唯一一次定义（函数内），但变量通过 `StoreVar`/`LoadVar` 走作用域槽（不是 SSA）。这种半 SSA 模式的优势是 lowering 简单——不用为每条控制流路径维护单独的变量版本。
>
> Phi 节点专门处理「需要根据前驱选值」的场景。常见的是「try 块的完成值」：try 体执行成功返回一个值，catch 也返回一个值，块外要根据是正常完成还是异常完成选不同的值。
>
> 物理上 Phi 由后端展开为 CLIF block parameters——见[控制流代码生成](../backend/control-flow-codegen.md)。
>
> </details>

## CFG 形状约束

`verify()`（`crates/wjsm-ir/src/verify.rs`）计算前驱、后继与支配集合，检查：

- `entry` 块存在。
- 所有终止器引用的块 id 在范围内。
- 每个 `ValueId` 的使用点被其定义点**支配**——定义在 `(block, instruction_index)` 粒度记录，同块内的先用后定也会被抓出。
- `Phi` 的 `sources` 与该块的实际前驱集合一致。
- 常量池里的 `FunctionRef` 指向存在的函数。

这些检查默认不跑，`--verify-ir` 或 `verify_ir_for_pipeline` 才触发。改 lowering 时应当开着它。

## 深入了解

- [语义层如何构造这些块](../frontend/control-flow-and-exceptions.md)
- [后端如何把 CFG 编成 CLIF](../backend/control-flow-codegen.md)
- [`verify()` 的完整不变量清单](validation-and-invariants.md)
