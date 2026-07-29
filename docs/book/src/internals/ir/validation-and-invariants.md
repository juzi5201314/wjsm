# IR 校验与不变量

这一章说明 `Program::verify()` 检查哪些不变量。它由 `--verify-ir` 触发，是 lowering 与后端之间的契约检查点。

## 调用位置

`crates/wjsm-cli/src/lib.rs` 的 `verify_ir_for_pipeline` 在 lower 阶段之后调用，只在 `--verify-ir` 时执行：

```rust
fn verify_ir_for_pipeline(program: &Program, verify_ir: bool) -> Result<()> {
    if verify_ir {
        program.verify().context("IR verification failed")?;
    }
    Ok(())
}
```

多模块路径同样在 `lower_bundle_with_options` 之后校验。默认不开启，因为它需要额外遍历全部函数并计算支配关系。

## 检查项

实现在 `crates/wjsm-ir/src/verify.rs`，`verify_module` 先校验模块级常量，再逐函数校验。

模块级：

- 每个 `Constant::FunctionRef` 指向的 `FunctionId` 必须存在。

函数级：

| 检查 | 失败信息 |
| --- | --- |
| entry 块存在 | `entry block bbN does not exist` |
| 非空块必须有真实终止器 | `block has instructions but its terminator is still unreachable` |
| 终止器目标块存在 | `terminator targets missing block bbN` |
| 指令与终止器引用的常量存在 | `references missing constant cN` |
| `home_object` 的 `FunctionId` 有效 | `invalid home_object function id @N` |
| phi 至少有一个来源 | `phi instruction has no sources` |
| entry 块不含 phi | `entry block must not contain phi instruction` |
| phi 来源必须是真实前驱 | 按前驱集合校验 |
| `super_call` 不能同时用 `forward_args` 与显式实参 | `super_call cannot combine forward_args with explicit args` |
| 值使用前必须已定义且支配当前位置 | `undefined value %N used by ...` |

错误统一包成 `IrVerificationError`，块级问题带 `block bbN: ` 前缀，便于定位。

## 支配关系检查

`verify.rs` 内部构造 `Predecessors`、`Successors`、`Definitions`、`Dominators` 四张表。非 phi 指令的每个操作数必须在支配当前块的块中定义；phi 的每个来源值必须在对应前驱块中可用。这条规则保证后端可以按块线性发射代码，不需要额外的活跃性修补。

## 什么时候该开

- 改动 lowering 的控制流构造（新循环形态、新 `try` 展开路径）。
- 新增终止器或 phi 生成点。
- 后端报出难以解释的类型或槽位错误时，先确认 IR 本身合法。

`fixtures/semantic/` 的 IR 快照与本校验互补：快照锁定输出形状，校验锁定结构合法性。

## 深入了解

- [IR 快照如何锁定 lowering 输出](../testing/semantic-snapshots.md)
- [`--verify-ir` 的用户侧说明](../../user/cli/global-options.md)
