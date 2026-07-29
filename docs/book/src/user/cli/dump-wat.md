# dump-wat

把编译产物以 WebAssembly 文本格式（WAT）打印出来，用于检查生成的指令序列。

```bash
wjsm dump-wat app.ts
wjsm dump-wat -e 'function foo() { return 1 }' --func foo
wjsm dump-wat -e 'const x = 1' --skeleton
```

## 选项

| 选项 | 说明 |
| --- | --- |
| `--func <NAME>` | 只打印指定函数体 |
| `--skeleton` | 只打印类型和函数签名，省略指令体 |
| `--root <DIR>` | 多文件入口先 bundling 再编译 |
| `--script` | 按 script 而不是 module 解析 |

`--skeleton` 适合快速看模块骨架：

```wat
(module
  (type $#type0 (;0;) (func (param i64)))
  (type $#type2 (;2;) (func (param i64 i64) (result i64)))
  (type $#type12 (;12;) (func (param i64 i64 i32 i32) (result i64)))
```

`--func` 按 WAT 中的函数名筛选。名字对不上时错误信息会列出可选项：

```text
Error: function 'nope' not found in WAT; available: $foo, $$module_main, $#func474, ...
```

用户函数名前缀为 `$`，模块顶层是 `$$module_main`，`$#funcN` 是没有名字的内部函数。

WAT 中大量 `i64` 参数是 NaN-boxed JavaScript 值的编码结果：所有 JS 值在 Wasm 层都表示为一个 `i64`。
带 `(param i64 i64 i32 i32)` 的 type 12 是原型方法调用约定，四个参数分别是环境、`this`、影子栈参数基址和参数个数。

`disasm` 对已有 `.wasm` 文件做同样的事，`dump-wat` 则从源码开始编译。

## 深入了解

- [NaN-boxed 值表示与标签编码](../../internals/backend/value-representation.md)
- [Import、Export 与主模块 ABI](../../internals/backend/imports-exports-and-abi.md)
- [函数、闭包与函数表的代码生成](../../internals/backend/functions-closures-and-table.md)
