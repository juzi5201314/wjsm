# IR 阶段

IR 不是独立的执行阶段，而是 lowering 的产物与后端的唯一输入。这一章说明 `Program` 在流水线中承担的契约。

## 位置

`wjsm-ir` 是零依赖 crate（6 个文件，约 3754 行），不引用 SWC、不引用 wasm-encoder。它定义的类型同时被 `wjsm-semantic`（生产者）、`wjsm-backend-wasm`（消费者）、`wjsm-module`（bundling 时合并多个 `Program`）、`wjsm-gc` 与 `wjsm-host` 引用。

`Program` 是 `Module` 的类型别名：

```rust
pub type Program = Module;
```

`Module` 持有四项状态：常量池 `Vec<Constant>`、函数表 `Vec<Function>`、`script_mode` 标记、可选的 `source_file`（供运行时错误堆栈映射）。

## 阶段间契约

| 方向 | 内容 |
| --- | --- |
| semantic → IR | 作用域已解析为 `$scope_id.name` 形式的变量名，TDZ 与 hoisting 已在 lowering 期决定 |
| IR → backend | 后端只读 `Program`，不回看 AST，也不做名称解析 |
| module → IR | 多模块 bundling 在 IR 层完成，通过 `offset_module_id` 平移 `ModuleId` 避免冲突 |

`offset_module_id` 使用 `checked_add`，溢出返回 `ModuleIdOffsetError` 而不是 panic——bundling 合并大量模块时这是硬边界。

## 校验点

`Program::verify()` 检查 IR 不变量，由 CLI 的 `--verify-ir` 触发，位置在 lowering 之后、codegen 之前：

```bash
wjsm run --verify-ir app.ts
wjsm build --stage lower --verify-ir -e 'const x = 1'
```

失败时 CLI 包装为 `IR verification failed`。这条路径默认关闭，因为它是 O(IR 大小) 的额外遍历。

## 稳定文本形式

`Module::dump_text()` 生成 `dump-ir` 的输出，也是 `fixtures/semantic/*.ir` 快照的比较对象。它必须对相同输入逐字节稳定，否则快照测试失去意义。

## 深入了解

- [Program、Module 与 Function 的结构](../ir/program-module-function.md)
- [IR 校验规则与不变量清单](../ir/validation-and-invariants.md)
- [稳定 dump 格式与快照约定](../ir/identifiers-and-display.md)
- [IR Program 的 bundling 合并方式](../modules/program-bundling.md)
