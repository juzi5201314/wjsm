# 诊断信息与流水线阶段

读懂 wjsm 报错的第一步是判断它来自哪个阶段。诊断文本本身带有阶段特征。

## 带源码位置的编译诊断

解析和 lowering 阶段的错误都带 `-->` 文件:行:列 以及源码摘录：

```text
Error: error: Expression expected
  --> input.ts:1:11
1 | const x = ;
  |           ^
```

同一格式覆盖语义检查：

```text
Error: error: cannot redeclare identifier `a` in the same scope
  --> input.ts:1:12
1 | let a = 1; let a = 2;
  |            ^^^^^^^^^^
```

TDZ 违规同样在编译期报出，不用等到运行：

```text
Error: error: cannot access `z` before initialisation
  --> input.ts:1:13
1 | console.log(z); let z = 1;
  |             ^
```

`-e` 的内联源码在诊断中显示为 `input.ts`。

## 运行时错误

未捕获异常在 stdout 打印 `Uncaught exception:`，在 stderr 打印 `Runtime error:`，退出码 2。堆栈相关信息取决于运行时能拿到的调试数据。

## 阶段进度与耗时

`-v` 打印阶段进入信息：

```text
Parsing...
Lowering to IR...
Compiling to WASM...
```

`--time` 打印各阶段耗时。不带 `-v` 用毫秒，带 `-v` 用微秒：

```bash
wjsm run -v --time -e 'console.log(1)'
```

```text
Timing: parse=285µs, lower=326µs, compile=1844µs, execute=16680µs
```

`execute` 只在实际执行的命令里出现。debug 构建的编译耗时明显高于 release 构建，横向比较请固定同一构建。

## 统计与 IR 自检

`--stats` 打印 IR 与产物规模：常量数、函数数、基本块数、指令数、WASM 字节数。`--verify-ir` 在 lowering 之后校验 IR 不变量，通过时无额外输出，失败时报错并终止。两者都可与任意子命令组合。

> <details><summary>为什么用不同前缀区分错误阶段？</summary>
>
> `Error: error: ...`（stderr 的 `Error:` + stdout 的 `error:`）和 `Runtime error: ...` 是两套不同的输出模式，对应两套不同的问题：
>
> - 编译错误：一定有源码位置。`Error: error: <message> --> <file>:<line>:<col>` 是 rustc 风格诊断，用户能在编辑器里直接跳转。
> - 运行时错误：没有源码位置（除非有 inspector 堆栈），直接描述发生了什么。
>
> 区分的好处是脚本可以按前缀做分流——比如编辑器集成时把 `Error: error:` 的位置显示成红色波浪线，把 `Runtime error:` 显示成通知。手动看的时候也能立刻知道「这是代码 bug 还是运行时问题」。
>
> </details>

## 深入了解

- [各阶段的划分方式与停止点语义](../../internals/pipeline/stage-isolation.md)
- [诊断文本与源码 span 的生成](../../internals/frontend/diagnostics-and-spans.md)
- [IR 校验规则与不变量](../../internals/ir/validation-and-invariants.md)
