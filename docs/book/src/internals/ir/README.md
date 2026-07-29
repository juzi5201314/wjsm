# 中间表示

`wjsm-ir` 是零依赖 crate，定义前端与后端之间唯一的契约。它不做优化，也不感知 WASM：语义层往里写，后端从里读。

- [Program、Module 与 Function](program-module-function.md)：顶层容器与函数元数据。
- [基本块与控制流图](cfg.md)：块、终止器与 CFG 形状约束。
- [Instruction 与 Constant](instructions-and-constants.md)：指令集与常量池。
- [Value、变量与类型信息](values-and-types.md)：`ValueId`、变量名与运算符枚举。
- [标识符、显示格式与稳定快照](identifiers-and-display.md)：`dump_text` 的稳定性要求。
- [IR 校验与不变量](validation-and-invariants.md)：`verify()` 强制的性质。
