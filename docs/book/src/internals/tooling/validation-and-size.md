# WASM 校验与尺寸分析

这一章说明 `validate` 和 `size` 命令的内部实现。

## validate

`validate` 命令校验 WASM 模块是否符合规范。它使用 `wasmparser::Validator` 验证 WASM 字节：

```rust
wasmparser::Validator::new()
    .validate_all(&wasm)
    .map_err(|error| ...)?;
```

`validate` 不执行模块，只检查结构合法性。它用于：

- 验证 `build` 产出的 WASM 是否合法。
- 验证 support module 的 WASM（build.rs 在生成后验证）。
- 调试 WASM 生成问题——如果 `validate` 失败，问题在 codegen 阶段。

公开 API `validate_wasm` 暴露这个能力给嵌入者。

## size

`size` 命令分析 WASM 模块的尺寸。它报告：

- 总字节数。
- 各 section 的尺寸（code、data、memory、function、table 等）。
- 函数数量、导入/导出数量等。

`wasm_section_sizes` 是公开 API，返回 section 级的尺寸明细。它用于：

- 监控 WASM 产物大小变化（回归检测）。
- 优化 codegen——找到最大的 section，优化目标明确。

## 与 build 的关系

`build` 命令在生成 WASM 后内部调用 `validate`，确保产物合法。`size` 是独立命令，需要先 `build` 得到 WASM，再 `size` 分析。

## 深入了解

- [源码输入与编译编排](source-input.md)
- [IR、AST、WAT 与反汇编工具](dump-and-disassembly.md)
- [用户侧的 validate](../../user/cli/validate.md)
