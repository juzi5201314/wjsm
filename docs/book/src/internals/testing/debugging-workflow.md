# 分层调试流程

这一章说明 wjsm 的分层调试方法论。

## 诊断阶段

AGENTS.md 定义了诊断流水线问题的阶段顺序：

```
parse → lower → module graph → codegen → host/runtime
```

问题定位时，从最早可能出问题的阶段开始检查，逐阶段向后推进。

## 工具

每个阶段有对应的诊断工具：

| 阶段 | 工具 | 输出 |
| --- | --- | --- |
| parse | `dump-ast` | SWC AST |
| lower | `dump-ir` | 语义 IR |
| module graph | `dump-ir`（看 bundle 后 IR） | bundle 后的 IR |
| codegen | `dump-clif` / `disasm` | CLIF / 反汇编 |
| host/runtime | fixture + 运行 | stdout/stderr |

比较相邻阶段的输出：如果 `dump-ast` 正确但 `dump-ir` 错误，问题在 lowering；如果 `dump-ir` 正确但 `dump-clif` 错误，问题在 codegen。

## 禁止临时日志

AGENTS.md 要求：不要在生产代码加临时日志，用 `dump-ast`、`dump-ir`、`dump-clif`、`disasm` 替代。如果这些工具无法暴露问题，再考虑其他手段。

## 命令示例

```bash
cargo run -- dump-ast -e 'const x = 1'
cargo run -- dump-ir -e 'const x = 1'
cargo run -- dump-clif -e 'const x = 1'
cargo run -- disasm /tmp/out.wjsm
```

`-e` 用于内联源码，文件路径用于多文件项目。

## 深入了解

- [IR、AST、WAT 与反汇编工具](../tooling/dump-and-disassembly.md)
- [阶段隔离与诊断输出](../pipeline/stage-isolation.md)
- [用户侧的调试与诊断工作流](../../user/workflows/debugging.md)
