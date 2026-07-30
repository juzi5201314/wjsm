# `repl`

进入交互式求值循环。每输入一行就编译执行一次。

```bash
wjsm repl
```

```text
wjsm> 1 + 1
2
wjsm> .exit
```

标准输入是终端时才打印 `wjsm> ` 提示符，所以管道输入不会污染输出：

```bash
printf '1+1\n2*3\n' | wjsm repl
```

## 退出

`.exit` 或 `.quit` 结束会话，读到 EOF（终端里按 `Ctrl-D`）同样结束。

## 每行都是独立表达式

默认模式下每一行都走 [`eval`](eval.md) 的路径，所以：

- 只能输入表达式，不能输入 `const x = 1;` 这类声明语句。
- 行与行之间**不共享状态**。上一行声明的变量在下一行不存在。
- 单行报错只打印诊断，会话继续。

```text
wjsm> const a = 5
Error: error: Expected ',', got 'ident'
wjsm> a * 2
Error: error: undeclared identifier `a`
```

需要跨语句的完整程序，写成文件用 [`run`](run.md)，或用 `run -e` 传一段完整源码。

## 选项

| 选项 | 作用 |
| --- | --- |
| `-e, --eval <EVAL>` | 只走一次求值路径然后退出，不进入循环 |
| `--script` | 按 script 而非 module 解析输入 |

`--script` 会改变求值路径：此时每行按完整源码执行（脚本模式），而不是包装成表达式打印。

> <details><summary>为什么默认 REPL 不共享状态？</summary>
>
> 「每行独立求值」的实现比「维护一个持续 session」简单得多——后者意味着每次输入都要在前一次的 IR 状态上做增量修改，而 IR 在 wjsm 里是 immutable 的，每次都重新生成。
>
> 一个折中方案是每次输入都包成「上次结果 → 变量名」的形式（比如 `$_`），但这会和用户代码里的标识符冲突，语义上也有歧义。Node 的 REPL 能共享状态是因为它本来就是一个长跑的 V8 isolate——增量更新对解释器是自然的，对 AOT 编译器不是。
>
> 实际使用上：REPL 是「快速验证一个表达式」的工具，不是「写多行程序」的工具。多行调试请用文件 + `wjsm run`。
>
> </details>

## 深入了解

- [REPL 与内联求值共用的编译路径](../../internals/tooling/source-input.md)
- [动态代码与 Eval 编译模式](../../internals/runtime-features/dynamic-code.md)
