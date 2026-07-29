# `eval`

计算一个表达式并打印结果。

```bash
wjsm eval '1 + 2 * 3'
```

```text
7
```

## 只接受表达式

`eval` 会把参数包进 `console.log((<CODE>))` 后编译执行，所以参数必须是一个**表达式**，不能是语句序列：

```bash
wjsm eval 'const x = 1; x + 1'
```

```text
Error: error: Expression expected
 --> input.ts:1:14
1 | console.log((const x=1; x+1))
  |              ^^^^^
```

诊断里的 `input.ts` 和列号来自包装后的代码，这是 `eval` 的正常表现。

需要执行语句、声明变量或运行多行程序时，用 `run -e`：

```bash
wjsm run -e 'const x = 1; console.log(x + 1)'
```

因为结果由 `console.log` 打印，输出格式与 `console.log` 一致：字符串不带引号，对象按 `console.log` 的渲染规则展开。

## 深入了解

- [表达式求值与编译编排的实现](../../internals/tooling/source-input.md)
