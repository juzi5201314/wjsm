# 解析、检查与 Lowering 问题

前端错误分两类：SWC 报出的语法错误，和 wjsm 语义层报出的绑定/早期错误。两者格式相同，都带源码片段和插入符。

## 语法错误

```text
Error: error: Expression expected
 --> input.ts:1:11
1 | const x = ;
  |           ^
```

`input.ts` 是 `-e` 内联代码的占位文件名；传文件时显示真实路径。

## 重复声明与 TDZ

```text
Error: error: cannot redeclare identifier `a` in the same scope
Error: error: cannot access `z` before initialisation
```

这些是 ECMAScript 早期错误，在 lowering 阶段报出，不需要执行程序就能发现。`wjsm check` 专门用于只做这一步。

## 内联代码中的 import 报「undeclared identifier」

```bash
wjsm run -e 'import fs from "node:fs"; console.log(typeof fs.readFileSync)'
```

会报 `undeclared identifier 'fs'`。`-e` 内联源码不参与模块图构建，`import` 声明不生效。把代码写进 `.mjs` 文件再运行。

## CommonJS 语法出现在 ESM 文件里

```text
Error: bundle entry c.mjs from root /tmp/x: Failed to lower modules:
error: undeclared identifier `module`
```

`.mjs` 强制按 ESM 处理，`module.exports` 在其中是未声明标识符。改用 `.cjs`，或改写为 `export`。

## await 被当作保留字

模块模式下 `await` 不能作标识符。若源码是旧式脚本，加 `--script`。

## 深入了解

- [Hoisting、TDZ 与早期错误](../../internals/frontend/hoisting-tdz-and-errors.md)
- [诊断与源码位置](../../internals/frontend/diagnostics-and-spans.md)
