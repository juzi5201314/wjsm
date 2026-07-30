# 阶段隔离与诊断输出

这一章说明为什么每个阶段都能单独停下来观察，以及排查问题时该用哪个出口。

## 阶段边界是可观察的

`--stage` 与 `dump-*` 命令不是调试开关，而是流水线本身的结构暴露：

| 出口 | 观察对象 | 实现 |
| --- | --- | --- |
| `--stage parse` / `dump-ast` | SWC AST（JSON） | `serde_json::to_string_pretty(ast)` |
| `--stage lower` / `dump-ir` | 语义 IR 文本 | `Program::dump_text` |
| `dump-wat` | 生成的 WAT | `wasmprinter` |
| `--stage compile` | WASM 字节 | 写文件或 stdout |
| `disasm` | 已有 `.wasm` 的 WAT | 与 `dump-wat` 共用打印器 |

这条约定的价值在于：定位问题时先确定哪一段的输出开始不对，而不是在整条链上猜。这也是 `AGENTS.md` 要求「诊断先定位失败阶段」的原因。

## 计时与统计

`PipelineTimings` 记录四段耗时（parse / lower / compile / execute），`--time` 触发打印。verbose 影响单位：

```text
Timing: parse=6ms, lower=10ms, compile=6ms, execute=67ms       # 默认
Timing: parse=285µs, lower=326µs, compile=1844µs, execute=…    # -v
```

`execute_us` 为 0 时该段不打印，因此 `build --stage compile --time` 只会看到前三段。

## IR 校验

`--verify-ir` 让 `verify_ir_for_pipeline` 在越过 lower 阶段前调用 `Program::verify()`。它检查 IR 结构不变量（块引用、终结符、值定义）。默认关闭是性能考虑；改动 lowering 或 IR 时应当开启。

## 不要用日志替代阶段出口

生产代码里不加临时日志是硬性约定。原因是这些出口已经覆盖了绝大多数需求：AST 不对看 `dump-ast`，IR 不对看 `dump-ir`，codegen 不对看 `dump-wat`，运行期不对用 `--inspect`。只有当这些路径都无法暴露问题时，才考虑其他手段。

> <details><summary>「不写临时日志」是不是过度约束？</summary>
>
> 看起来是。日志便宜，加几行就能看到运行时状态。
>
> 但实际工程里「临时日志」有个通病：它留下来就删不掉。理由是「调试还没结束，再多打一点」「这个 warning 也许以后用得上」。半年后代码里满是「以前调试用」的日志，没人记得哪条是关键的，PR review 时也懒得删。
>
> 阶段出口的替代思路：把「看内部状态」从「改代码加日志、运行、看输出、删日志」变成「运行 dump-* 命令、看输出」。不需要改代码，没有删除负担，输出有标准化格式（json、IR 文本、WAT）。
>
> 短期成本：dump 命令的输出比临时日志啰嗦，你需要写点小脚本过滤。
>
> 长期收益：项目永远干净，没有「历史遗留日志」这种东西。
>
> </details>

## 深入了解

- [dump 与反汇编工具的实现](../tooling/dump-and-disassembly.md)
- [IR 校验规则与不变量清单](../ir/validation-and-invariants.md)
- [分层调试的推荐顺序](../testing/debugging-workflow.md)
- [用户侧的诊断输出说明](../../user/output/diagnostics.md)
