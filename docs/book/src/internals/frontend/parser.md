# SWC 解析边界

这一章说明 `wjsm-parser` 的职责范围：它只做「源码 → SWC AST」，不碰语义。

## crate 规模与依赖

`wjsm-parser` 是 workspace 里最小的 crate（2 个文件，264 行），只依赖 `anyhow` 与 `swc_core`。它不依赖 `wjsm-ir`，因此不参与任何语义决策。

- `lib.rs`：语法选择与解析入口
- `diagnostic.rs`：rustc 风格诊断格式化

## 语法模式由扩展名决定

`syntax_for_path` 按扩展名选择 SWC 语法配置：

| 扩展名 | 语法 | JSX |
| --- | --- | --- |
| `.ts` | `Syntax::Typescript` | 关 |
| `.tsx` | `Syntax::Typescript` | 开 |
| `.jsx` | `Syntax::Es` | 开 |
| `.js` / `.mjs` / `.cjs` | `Syntax::Es` | 关 |
| 其他 / 无扩展名 | `Syntax::Typescript` | 开 |

两种语法都开启 `decorators`。`Syntax::Es` 额外设置 `decorators_before_export` 与 `allow_super_outside_method`。

无扩展名时回落到「TypeScript + TSX」，这是最宽的组合，因此 `-e` 内联源码与 stdin 输入都按它解析。`parse_module` 用的诊断文件名是 `input.ts`，与用户看到的错误信息一致。

## script 与 module

`parse_module_inner` 的 `script` 参数决定调用 `parse_script` 还是 `parse_module`。script 模式解析完成后，`Script` 被包装成 `Module`（`body` 逐项转 `ModuleItem::Stmt`），后续阶段只处理一种 AST 类型。

这个包装是 script 模式能复用整条 lowering 链的原因，也是 `--script` 只影响解析器而不影响后端的原因。

## 错误格式化

解析失败时错误经 `diagnostic::format_parse_error` 转成带行列、源码片段与 caret 的文本：

```text
error: Expression expected
 --> input.ts:1:11
1 | const x = ;
  |           ^
```

`format_byte_diagnostic` 是公开的，语义层复用它输出同样格式的诊断，因此前端两个阶段的错误在用户看来是一致的。

## 不做的事

- 不做类型检查。TypeScript 类型语法参与解析并在 lowering 阶段擦除，类型正确性不校验。
- 不做作用域分析。`parse_module` 返回的 AST 里标识符没有绑定信息。
- 不做转换。JSX、装饰器、TS 语法的处理都在 `wjsm-semantic`。

## 深入了解

- [语义 Lowering 如何消费 AST](two-phase-lowering.md)
- [诊断与 span 的传递路径](diagnostics-and-spans.md)
- [解析阶段在流水线中的位置](../pipeline/parse.md)
