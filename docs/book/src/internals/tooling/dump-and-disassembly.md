# IR、AST 与反汇编工具

这一章说明 `dump-ast`、`dump-ir`、`dump-clif` 和 `disasm` 命令的内部实现。

## 共用管道

四个工具共用同一条输入管道——读取源码或 artifact、按目标阶段执行、把中间结果格式化输出。差别只在「执行到哪个阶段」和「用什么格式化器」。

| 命令 | 停止阶段 | 格式化器 |
| --- | --- | --- |
| `dump-ast` | parse | `serde_json::to_string_pretty` |
| `dump-ir` | lower | `Program::dump_text` |
| `dump-clif` | native codegen | Cranelift IR 文本 |
| `disasm` | native image | 机器码反汇编 |

## dump-ir 的过滤

`dump-ir --func <NAME>` 只输出指定函数，同时附上常量池，让 `cN` 引用仍可解析。`--format dot` 输出 Graphviz 图，用 `dot -Tsvg` 渲染控制流。

`--func` 匹配的是 IR 中的函数名（`fn @foo`）。找不到时报 `function 'nope' not found`，退出码 1。

## dump-clif 的输入

`dump-clif` 接受三种输入：

1. 源码文件（先 parse → lower → CLIF 编译）；
2. 内联源码 `-e`；
3. portable `.wjsm` artifact（先 bounded decode + verify → CLIF 编译）。

源码输入支持 `--root`、`--script`。artifact 输入走 bounded decode 与 verification。

## disasm 的输入

`disasm` 只接受 portable `.wjsm`。它先验证 artifact，再按当前宿主 ISA 编译为 native image，然后反汇编机器码。输出绑定当前 target、CPU feature 与 codegen settings，不能作为跨平台 artifact。

## 深入了解

- [阶段隔离与诊断输出](../pipeline/stage-isolation.md)
- [标识符、显示格式与稳定快照](../ir/identifiers-and-display.md)
- [分层调试流程](../testing/debugging-workflow.md)
- [`dump-ir` 的用户侧用法](../../user/cli/dump-ir.md)
