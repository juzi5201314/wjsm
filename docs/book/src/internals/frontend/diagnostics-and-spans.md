# 诊断与源码位置

这一章说明诊断消息的生成链路：从 SWC span 到最终终端上那段带 caret 的文本。

## 两类诊断

解析错误由 `wjsm-parser` 生成，语义错误由 `wjsm-semantic` 生成，两者最终格式一致。

`wjsm-parser` 的 `diagnostic.rs` 提供 `format_byte_diagnostic`，输入是字节偏移，输出 rustc 风格文本：

```text
error: Expression expected
 --> input.ts:1:11
1 | const x = ;
  |           ^
```

行列由 SWC `SourceMap::lookup_char_pos` 计算，列号是 UTF-8 字节偏移加一。caret 长度取 `end - start`，单点错误退化为一个 `^`。

## 语义诊断

`wjsm-semantic` 定义 `LoweringError` 与 `Diagnostic`：

```rust
pub enum LoweringError { Diagnostic(Diagnostic), /* ... */ }

pub struct Diagnostic {
    // 字节区间 + 消息
}
```

`Diagnostic` 实现 `Display`，在有源码上下文时复用 parser 的格式化逻辑输出片段和 caret，无上下文时退化为纯消息。`Lowerer::error(span, message)` 是构造入口，各 lowering 子模块通过它报错。

## 源码上下文的传递

`lower_module_with_source` 接收 `Option<Arc<str>>` 源文本与 `filename`：

- 有源文本：诊断带源码片段。
- 无源文本：只有消息和位置。

CLI 的 `lower_parsed_module` 总是传入源文本，并在缺少文件名时按 `--script` 选择 `input.js` 或 `input.ts` 作为展示名。这就是内联源码报错里出现 `input.ts` 的原因。

## SourceSpan 与运行时映射

IR 层的 `SourceSpan` 是另一套结构，用于运行时错误堆栈，而非编译期诊断：

```rust
pub struct SourceSpan {
    pub line: u32,  // 1-indexed
    pub col: u32,   // 1-indexed，UTF-8 字节偏移
}
```

它随 IR 指令保存，供 inspector 断点和运行时堆栈映射使用。`emit_debug_checks` 为真时，lowering 在语句入口发射 `DebugCheck` 指令并附带 `SourceSpan`；此时必须提供源文本，否则行列无法解析，指令被跳过。

## 深入了解

- [解析阶段的语法选择](parser.md)
- [Inspector 如何消费 SourceSpan](../runtime-features/inspector-and-cdp.md)
- [用户侧的诊断格式](../../user/output/diagnostics.md)
