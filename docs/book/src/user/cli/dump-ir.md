# dump-ir

打印语义 lowering 之后的 IR。判断某段代码「编译成了什么」时，这是最直接的入口。

```bash
wjsm dump-ir app.ts
wjsm dump-ir -e 'const x = 1'
wjsm dump-ir -e 'function foo() { return 1 }' --func foo
wjsm dump-ir app.js --format dot > /tmp/ir.dot
```

## 输出格式

文本格式先列常量池，再按函数列基本块和指令：

```text
module {
  constants:
    c0 = undefined
    c20 = number(1)

  fn @$module_main [entry=bb0]:
    bb0:
      %0 = const c0
      store var $0.undefined, %0
      %21 = const c20
      store var $0.x, %21
      return
}
```

几个读法要点：

- `$module_main` 是模块顶层代码被包装成的函数。
- `%N` 是 IR 值编号，`cN` 引用常量池条目，`bbN` 是基本块。
- 变量名形如 `$0.x`，`$0` 是作用域编号，也就是「作用域 0 中的 `x`」。
- 常量池开头那批 `Math.PI`、`Number.EPSILON` 等是每个模块都会写入的内建初始化，不是你的代码产生的。

## 选项

| 选项 | 说明 |
| --- | --- |
| `--format <text\|dot>` | `text` 为默认；`dot` 输出 Graphviz 图，用 `dot -Tsvg` 渲染控制流 |
| `--func <NAME>` | 只打印指定函数，同时保留常量池，让 `cN` 仍可解析 |
| `--root <DIR>` | 多文件入口先 bundling 再 dump |
| `--script` | 按 script 而不是 module 解析 |

`--func` 匹配的是 IR 中的函数名。找不到时报 `function 'nope' not found`，退出码 1。
函数名可以先用不带 `--func` 的输出确认，例如 `fn @foo [needs_prototype] ...`。

终端有颜色时输出会着色，`--no-color` 或重定向到文件时是纯文本。

同样的 IR 也可以用 `wjsm build --stage lower` 得到。

## 深入了解

- [IR 的 Program、Module 与 Function 结构](../../internals/ir/program-module-function.md)
- [标识符命名规则与稳定 dump 格式](../../internals/ir/identifiers-and-display.md)
- [基本块与控制流图的构造](../../internals/ir/cfg.md)
