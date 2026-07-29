# 新增语言功能

这一章说明如何向 wjsm 添加新的 JavaScript 或 TypeScript 语法支持。

## 步骤

1. **解析器**：`wjsm-parser` 检查 SWC 是否已支持该语法。SWC 通常支持已标准化的语法，但提案阶段语法可能需要额外配置。如果 SWC 支持，只需在 parser 边界暴露它。
2. **语义 Lowering**：`wjsm-semantic` 添加对新语法的 lowering。如果是表达式/语句，在 `expressions.rs` / `statements.rs` 添加处理；如果是声明，在 `declarations.rs` 添加。
3. **IR**：如果新语法需要新的 IR 指令，在 `wjsm-ir` 添加（见[新增或修改 IR](changing-ir.md)）。
4. **后端**：`wjsm-backend-wasm` 添加新 IR 指令的 codegen（如果需要）。
5. **运行时**：如果新语法需要运行时支持（如 `Symbol` 的 well-known symbol），在 `wjsm-builtins` 或 host-wasm 添加。
6. **测试**：添加 fixture 验证行为，添加 IR 快照验证 lowering。

## 示例：添加 `??=` 空值赋值

1. SWC 已支持 `??=`。
2. `wjsm-semantic` 在 `expressions.rs` 的 assignment 处理里加 `LogicalAndAssign` 分支，lowering 成「读取 lhs → 如果是 null/undefined 则计算 rhs 并赋值」的 IR。
3. 不需要新 IR 指令——复用 `Assign`、`Branch` 等。
4. 不需要后端改动——现有 IR 指令已有 codegen。
5. 不需要运行时改动——null/undefined 检查是值层操作。
6. 添加 `fixtures/happy/nullish_assign.js` + `.expected`。

## 早期错误

新语法可能有早期错误约束（如 `const` 的 TDZ）。在 `wjsm-semantic` 的 early error pass 里实现，添加 `fixtures/errors/` 验证错误行为。

## 深入了解

- [新增或修改 IR](changing-ir.md)
- [新增 Builtin](adding-builtins.md)
- [语义 Lowering 阶段](../pipeline/lower.md)
