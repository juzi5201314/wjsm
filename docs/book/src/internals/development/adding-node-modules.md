# 新增 Node.js 模块

这一章说明如何向 wjsm 添加新的 `node:` 前缀内置模块。

## 步骤

1. **注册 specifier**：`wjsm-module/src/builtin_modules.rs` 的内置模块表添加新模块名。`lookup` 在 strip `node:` 前缀后查表。
2. **选择实现位置**：
   - Rust 实现的模块：在 `wjsm-host-wasm/src/` 添加 `runtime_node_<module>.rs`。
   - JS polyfill：在 `wjsm-module/builtin_js/` 添加 `node_<module>.js`。
   - 混合：Rust 提供核心能力，JS polyfill 包装 API。
3. **实现 API**：按 Node.js 官方文档实现模块的 API。可以参考 Node.js 源码，但 wjsm 的实现基于自身运行时能力，不是 Node.js 的移植。
4. **条件解析**：`node` 条件已在解析顺序中，`node:` 模块会自动被识别。
5. **测试**：添加 fixture（如果行为可测）或集成测试。

## 24 个内置模块

现有 24 个内置模块的清单见 `wjsm-module/src/builtin_modules.rs`。新模块添加后更新这个表。

## 与 Web API 的关系

部分 Node.js 模块与 Web API 共享实现。例如 `node:fetch` 与 `fetch()` 使用同一个 `wjsm-builtins/src/fetch/` 实现。新增模块时先检查是否能复用 Web API 实现，避免重复。

## 条件解析

`node:` 模块在包解析的条件顺序中位于 `wjsm` → `browser` → 自定义 → `node` → import/require → default。`wjsm` 条件永远首位，遮蔽其他——wjsm 的内置模块优先于 npm 上的同名包。

## 深入了解

- [Node.js Built-in 模块组织](../runtime-features/node-builtins.md)
- [包解析、条件与 browser 映射](../modules/package-conditions.md)
- [新增 Builtin](adding-builtins.md)
