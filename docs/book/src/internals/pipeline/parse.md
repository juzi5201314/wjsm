# 解析阶段

`wjsm-parser` 是唯一的 SWC 接触点，只有 264 行两个文件（`lib.rs`、`diagnostic.rs`）。它的职责是把源码变成 `swc_ast::Module`，并把 SWC 的错误转成 rustc 风格诊断。

## 语法模式按扩展名选择

`syntax_for_path` 决定用哪种 SWC 语法：

| 扩展名 | 语法 | JSX |
| --- | --- | --- |
| `.ts` | `Syntax::Typescript` | 否 |
| `.tsx` | `Syntax::Typescript` | 是 |
| `.jsx` | `Syntax::Es` | 是 |
| `.js` / `.mjs` / `.cjs` | `Syntax::Es` | 否 |
| 其他/未知 | `Syntax::Typescript` | 是 |

两种语法都开启 `decorators`。ES 语法额外开启 `decorators_before_export` 和 `allow_super_outside_method`。没有文件名的入口（`-e`、stdin）落到默认分支，即 TypeScript + JSX，这是最宽松的组合，所以内联源码可以直接写 TS 语法。

## Script 与 Module

`parse_module_inner` 的 `script` 参数决定调用 `parse_script` 还是 `parse_module`。script 模式解析完成后把 `Script` 包装成 `Module`：

```rust
Ok(swc_ast::Module {
    span: script_ast.span,
    body: script_ast.body.into_iter().map(swc_ast::ModuleItem::Stmt).collect(),
    shebang: script_ast.shebang,
})
```

后续阶段因此只需处理一种 AST 形态。差别体现在语义层：module 模式下 `await` 是保留字，script 模式下可作标识符。

## 诊断格式

`diagnostic.rs` 提供 `format_byte_diagnostic`，把字节偏移转成行列并渲染源码片段与 caret：

```text
error: Expression expected
 --> input.ts:1:11
1 | const x = ;
  |           ^
```

这个函数是公开的，语义层的错误也复用它，所以解析错误和 lowering 错误的输出格式一致。位置转换依赖 SWC 的 `SourceMap::lookup_char_pos`。

## 相关章节

- [SWC 解析边界](../frontend/parser.md)
- [诊断与源码位置](../frontend/diagnostics-and-spans.md)
- [用户视角的输入模式](../../user/getting-started/input-modes.md)
