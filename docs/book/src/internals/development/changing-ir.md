# 新增或修改 IR

这一章说明如何向 `wjsm-ir` 添加新指令或修改现有 IR。

## 何时需要新指令

新指令的引入要谨慎——IR 指令影响 codegen、验证、快照等多个层。优先考虑：

1. **复用现有指令**：现有指令的组合能否表达新语义？
2. **参数化现有指令**：给现有指令加参数是否足够？
3. **新指令**：前两条都不行时才引入。

## 添加步骤

1. **定义 Instruction variant**：在 `wjsm-ir/src/instructions.rs` 添加新 variant。
2. **显示格式**：实现 `Display`，确保稳定输出（快照依赖）。
3. **类型检查**：在 IR 验证 pass 添加新指令的类型规则。
4. **code lower**：`wjsm-semantic` 的 lowering 发射新指令。
5. **codegen**：`wjsm-backend-wasm` 为新指令添加 WASM 生成。
6. **快照**：更新 IR 快照，审查 diff。
7. **测试**：添加 fixture 和 IR 快照测试。

## 稳定快照

IR 的文本格式必须稳定。新指令的 `Display` 实现要：

- 输出格式明确，不依赖指针地址或随机顺序。
- 参数按固定顺序排列。
- 未来的改动不应改变已有指令的输出格式。

`WJSM_UPDATE_SNAPSHOTS=1` 更新快照，但更新前要审查 diff。

## IR 校验

`wjsm-ir/src/validation.rs` 检查 IR 的不变量。新指令需要在这里添加校验规则，确保：

- 操作数类型正确。
- 基本块结构合法。
- 值的定义和使用匹配。

## 深入了解

- [Instruction 与 Constant](../ir/instructions-and-constants.md)
- [IR 校验与不变量](../ir/validation-and-invariants.md)
- [标识符、显示格式与稳定快照](../ir/identifiers-and-display.md)
