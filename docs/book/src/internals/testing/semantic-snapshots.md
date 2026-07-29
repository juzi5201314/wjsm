# 语义 IR 快照

这一章说明语义 IR 的快照测试机制。

## 快照测试

`wjsm-semantic` 的 lowering 改动需要 IR 快照测试。快照测试比较 lowering 后的 IR 文本输出与期望快照，确保 lowering 行为不意外变化。

```bash
WJSM_UPDATE_SNAPSHOTS=1 cargo nextest run -p wjsm-semantic -- lowering_snapshots
```

## 稳定显示

IR 的文本格式必须稳定——相同的 IR 结构总是产生相同的文本输出。`identifiers-and-display.md` 说明标识符和显示格式的稳定性保证。

快照内容包含：Program、Module、Function、CFG（基本块和边）、Instruction、Constant、Value 等。每个元素有稳定的文本表示。

## 何时更新

lowering 改动后，如果快照变化是预期的（例如新增了 IR 指令），用 `WJSM_UPDATE_SNAPSHOTS=1` 更新。更新前要审查 diff——确认变化是预期行为，不是 bug。

## 与 fixture 的区别

| 机制 | 对象 | 阶段 |
| --- | --- | --- |
| Fixture | 用户可见输出（stdout） | 端到端 |
| IR 快照 | 语义 IR 文本 | lowering 后 |

Fixture 测试端到端行为，IR 快照测试中间表示。lowering 回归可能不影响最终输出（后续阶段补偿了），但 IR 快照能捕获中间阶段的变化。

## 深入了解

- [标识符、显示格式与稳定快照](../ir/identifiers-and-display.md)
- [语义 Lowering 阶段](../pipeline/lower.md)
- [IR 校验与不变量](../ir/validation-and-invariants.md)
