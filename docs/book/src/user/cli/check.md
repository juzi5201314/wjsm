# `check`

只做解析和语义检查，不生成 WASM、不执行程序。

```text
wjsm check [OPTIONS] [INPUT]
```

`check` 走完流水线的 parse 和 lower 两个阶段。这意味着它能发现的不只是语法错误，还包括 lowering 阶段判定的早期错误，例如重复声明、`await` 在模块顶层被当作标识符、赋值给 `const`。

```bash
wjsm check src/main.ts
wjsm check -e 'const x = 1'
```

无错误时安静退出，退出码 0：

```bash
$ wjsm check -e 'const x = 1'
$ echo $?
0
```

有错误时把诊断写到标准错误，退出码 1：

```text
$ wjsm check -e 'const x = ;'
Error: error: Expression expected
 --> input.ts:1:11
1 | const x = ;
  |           ^
```

## 与类型检查的区别

TypeScript 的类型标注会被解析并丢弃，`check` 不做类型推导或类型校验。`const x: number = "s"` 能通过 `check`。需要类型检查请用 `tsc`。

## 相关选项

`--root` 让入口按模块图检查（会连带解析依赖）；`--script` 用脚本模式解析。`-v` 打印阶段进度。

## 深入了解

- [两阶段 Lowering 为什么能在不执行代码时发现早期错误](../../internals/frontend/two-phase-lowering.md)
- [Hoisting、TDZ 与早期错误的判定位置](../../internals/frontend/hoisting-tdz-and-errors.md)
