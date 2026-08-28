# Node.js Built-in 模块组织

这一章说明 `node:` 前缀模块的组织和实现。

## 41 个内置模块

`crates/wjsm-module/src/builtin_modules.rs` 定义 41 个 Node.js 内置模块。specifier 以 `node:` 前缀标识，`lookup` 时 strip 前缀后查表。

内置模块分布在三个位置：

| 位置 | 内容 |
| --- | --- |
| `wjsm-host-native/src/runtime_node_*.rs` | Rust 实现的模块（fs、child_process 等） |
| `wjsm-module/builtin_js/*.js` | JS polyfill（worker_threads 等） |
| `wjsm-builtins/src/` | 共享算法（fetch、streams 等） |

## 模块查找

`builtin_modules.rs` 的 `lookup(specifier)` 在 `node:` strip 后查表。找到的模块返回类型（Rust 原生 / JS polyfill / 混合），运行时按类型加载。

测试验证了 `node:path` 能查到，`node:not_real` 查不到会返回错误。

## 条件解析

`node:` 模块在包解析的条件顺序中位于 `wjsm` → `browser` → 自定义 → `node` → import/require → default。`node` 条件让 `node:` 模块在 ESM 解析时被识别为内置模块，不走文件系统查找。

## 与 Web API 的关系

部分 Node.js 模块与 Web API 共享实现。例如 `node:fetch` 与 `fetch()` 使用同一个 `wjsm-builtins/src/fetch/` 实现。`node:stream` 与 Web Streams 通过适配层互操作。

## 深入了解

- [包解析、条件与 browser 映射](../modules/package-conditions.md)
- [核心 JavaScript Builtins 的分域组织](../host-runtime/javascript-builtins.md)
- [用户侧的 Node.js 兼容能力](../../user/runtime/node-compatibility.md)
