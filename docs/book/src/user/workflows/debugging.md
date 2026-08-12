# 调试与诊断工作流

wjsm 的诊断思路是「逐阶段定位」：从最早可能出问题的阶段开始检查，比较相邻阶段的输出，确定问题出在哪一段。

## 诊断阶段顺序

```text
dump-ast → dump-ir → dump-clif → disasm
```

| 阶段 | 工具 | 输出 |
| --- | --- | --- |
| 解析 | [`dump-ast`](../cli/dump-ast.md) | SWC AST（JSON） |
| 语义 lowering | [`dump-ir`](../cli/dump-ir.md) | 语义 IR 文本 |
| native lowering | [`dump-clif`](../cli/dump-clif.md) | Cranelift IR |
| 机器码 | [`disasm`](../cli/disasm.md) | 反汇编 |

比较相邻阶段的输出：

- AST 正确而 IR 错误 → 问题在 semantic lowering。
- IR 正确而 CLIF 错误 → 问题在 native lowering。
- CLIF 正确而机器码 / relocation 错误 → 看 `disasm` 与 image loader。

```bash
wjsm dump-ast app.ts
wjsm dump-ir app.ts
wjsm dump-clif app.ts           # 也接受 .wjsm artifact
wjsm disasm /tmp/app.wjsm       # 只接受 portable artifact
```

`disasm` 的输出绑定当前 target、CPU feature 与 codegen settings，不能跨平台。定位问题时先比较 `dump-ir` 与 `dump-clif`；只有 CLIF 正确而最终指令异常时再看反汇编。

## IR 校验

`--verify-ir` 在 lowering 之后、继续 codegen 之前调用 `Program::verify()`，检查 IR 结构不变量（块引用、终结符、值定义）：

```bash
wjsm run --verify-ir app.ts
```

默认关闭是性能考虑。改动 lowering 或 IR 逻辑时应当开启。

## CDP 调试

`--inspect` 启动 Chrome DevTools Protocol 调试器，`--inspect-brk` 启动并在入口处暂停：

```bash
wjsm --inspect run app.ts
wjsm --inspect-brk run app.ts
wjsm --inspect-brk=127.0.0.1:9229 run app.ts
```

用 Chrome DevTools 或任何 CDP 客户端连接到指定端口。`--inspect-brk` 会在执行用户代码前暂停，方便在入口设断点。

`--inspect` 同时驱动 lowering 发射 `DebugCheck` 指令和 native codegen 生成调试段——两侧必须同时开启，断点才能映射回源码位置。

## 阶段进度与耗时

`-v` 打印阶段进入信息，`--time` 打印各阶段耗时：

```bash
wjsm run -v --time app.ts
```

```text
Parsing...
Lowering to IR...
Timing: parse=285µs, lower=326µs, compile=1844µs, execute=16680µs
```

`-v` 让计时用微秒精度。debug 构建的编译耗时明显高于 release 构建，横向比较请固定同一构建。

## 快速验证

### check

[`check`](../cli/check.md) 只走 parse 和 lower 两个阶段，不生成代码也不执行。能发现语法错误、重复声明、TDZ 访问等早期错误：

```bash
wjsm check src/main.ts --root .
wjsm check -e 'const x = 1'
```

无错误时安静退出，退出码 0。适合在 CI 或开发时快速确认代码能否通过编译。

### lint

[`lint`](../cli/lint.md) 做 AST 级规则检查，三条规则：`eqeq`、`neqeq`、`debugger-noop`。有任何命中时退出码 1：

```bash
wjsm lint app.ts
```

`lint` 不判断代码能否编译，要确认能否编译用 `check`。

> <details><summary>为什么不用临时日志？</summary>
>
> wjsm 的约定是：生产代码里不加临时日志，用 `dump-ast`、`dump-ir`、`dump-clif`、`disasm` 替代。这些工具已经覆盖了绝大多数调试需求——看 AST 用 `dump-ast`，看 IR 用 `dump-ir`，看 native codegen 用 `dump-clif`，运行期问题用 `--inspect`。
>
> 临时日志的问题是删不掉：调试完了没人记得删，半年后代码里满是「以前调试用」的日志。阶段出口把「看内部状态」从「改代码加日志、运行、看输出、删日志」变成「运行 dump 命令、看输出」，不需要改代码。
>
> 只有当这些路径都无法暴露问题时，才考虑其他手段。
>
> </details>

## 深入了解

- [dump-ast](../cli/dump-ast.md)
- [dump-ir](../cli/dump-ir.md)
- [dump-clif](../cli/dump-clif.md)
- [disasm](../cli/disasm.md)
- [分层调试流程](../../internals/testing/debugging-workflow.md)
- [阶段隔离与诊断输出](../../internals/pipeline/stage-isolation.md)
