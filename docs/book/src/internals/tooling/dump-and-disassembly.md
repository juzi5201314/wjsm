# IR、AST、WAT 与反汇编工具

这一章说明 `dump-ast`、`dump-ir`、`dump-wat` 和 `disasm` 命令的内部实现。

## 命令对应阶段

| 命令 | 输出 | 对应阶段 |
| --- | --- | --- |
| `dump-ast` | SWC AST | 解析后 |
| `dump-ir` | 语义 IR | lowering 后 |
| `dump-wat` | WAT 文本 | WASM 编码后 |
| `disasm` | 反汇编 | WASM 编码后（带解析） |

`ir_output.rs` 实现这些命令的输出格式化。它们是诊断工具——让开发者看到相邻阶段的输出，定位问题出在哪一步。

## dump-ast

`dump-ast` 输出 SWC 解析器生成的 AST。这是流水线的第一步输出，用于验证解析器是否正确理解源码。

## dump-ir

`dump-ir` 输出语义 IR。IR 是 wjsm 的中间表示（见[中间表示](../ir/README.md)），`dump-ir` 输出 IR 的文本格式，包含 Program、Module、Function、CFG、Instruction 等。

IR 有稳定快照机制（见[标识符、显示格式与稳定快照](../ir/identifiers-and-display.md)），`dump-ir` 的输出可以用于快照测试。

## dump-wat 与 disasm

`dump-wat` 把 WASM 模块转成 WAT 文本格式。`disasm` 也输出 WAT，但附带额外解析——例如解析函数索引对应的 import 名、解析类型签名等。

这两个命令是 WASM 层的诊断工具。lowering 问题用 `dump-ir`，codegen 问题用 `dump-wat` / `disasm`。

## 诊断流程

AGENTS.md 的诊断流程建议：`dump-ast` → `dump-ir` → `dump-wat` → `disasm`，比较相邻阶段输出定位问题。不要在生产代码加临时日志，用这些工具替代。

## 深入了解

- [标识符、显示格式与稳定快照](../ir/identifiers-and-display.md)
- [阶段隔离与诊断输出](../pipeline/stage-isolation.md)
- [用户侧的 dump-ast](../../user/cli/dump-ast.md)
