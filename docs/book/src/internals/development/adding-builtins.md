# 新增 Builtin

这一章说明如何向 wjsm 添加新的 JavaScript 内置函数或方法。

## 三层协作

新增 builtin 涉及三层：

| 层 | crate | 职责 |
| --- | --- | --- |
| 语义拦截 | `wjsm-semantic` | 识别已知调用形态，发射 `CallBuiltin` |
| codegen | `wjsm-backend-wasm` | `Builtin` → WASM function index |
| 运行时 | `wjsm-builtins` / `wjsm-host-wasm` | 实现算法，注册到 Linker |

## 步骤

1. **算法实现**：在 `wjsm-builtins` 的对应域文件（如 `object_builtins.rs`、`string_methods.rs`）添加算法函数。函数签名接受 `ExecContext` 泛型，通过 ctx 访问对象堆和 GC。
2. **Builtin variant**：在 `wjsm-ir` 或 `wjsm-semantic` 的 `Builtin` enum 添加新 variant。
3. **语义拦截**：`wjsm-semantic/src/builtins.rs` 识别调用形态，发射 `CallBuiltin(NewVariant)`。
4. **codegen 分派**：`compiler_builtins_*.rs` 添加新 variant 的处理，决定 `BuiltinDispatch::Handled`（完整调用序列）或 `NeedsFallback`。
5. **host import 注册**：`runtime_linker.rs` 把 Rust 函数注册到 Linker。
6. **测试**：添加 fixture 验证行为。

## 何时拦截

语义拦截的性能收益：跳过属性查找、原型链遍历、方法绑定。对于频繁调用的方法（如 `Array.prototype.push`），拦截有显著收益。对于冷门方法，可以直接走属性查找路径。

## 返回值

`dest: Option<ValueId>` 为 `None` 时 builtin 只有副作用，不产生值。codegen 跳过结果赋值。`BuiltinDispatch::Handled` 表示 codegen 已生成完整调用序列。

## 深入了解

- [NativeCallable 注册表](../host-runtime/native-callables.md)
- [核心 JavaScript Builtins 的分域组织](../host-runtime/javascript-builtins.md)
- [新增 Host Import](adding-host-imports.md)
