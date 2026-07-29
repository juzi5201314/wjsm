# Host Import 注册与包装层

这一章说明约 507 个 `env.*` host 函数如何注册到 wasmtime Linker。

## host_import_registry

`crates/wjsm-backend-wasm/src/host_import_registry/` 用六个 `specs_part*.rs` 文件定义 host import 规格。每个 spec 记录 import 名、type index 和特殊标记。

规格表是 codegen 侧的「需要哪些 import」声明，host-wasm 侧的 `runtime_linker.rs` 负责把实际 Rust 函数注册到 `wasmtime::Linker`。两侧通过 import 名和 type index 对齐。

## 注册流程

`runtime_linker.rs` 的 `register_host_imports` 遍历规格表，对每个 import 调用 `linker.define`：

```rust
linker.define("env", "console_log", func_wrap(...))?;
linker.define("env", "obj_get", func_wrap(...))?;
```

`func_wrap` 的具体形式由 type index 决定：Type 12 的 import 用 `Func::wrap` 包成 `(i64, i64, i32, i32) -> i64`，Type 7 的用 `(i32) -> i32`，依此类推。

## NativeCallable 注册表

`runtime_builtins.rs` 维护 `NativeCallable` 注册表，把 IR 的 `Builtin` enum 变体映射到具体 Rust 函数。语义层发射 `CallBuiltin` 时携带 Builtin variant，后端在 codegen 阶段查表得到 WASM function index，生成 `Call` 指令。

## 包装层的薄度

host import 函数大多是薄包装：从 `Caller<RuntimeState>` 取出状态，调用 `wjsm-builtins` 的泛型算法，把结果编码回 NaN-box 值。语义逻辑在 builtins 层，不在包装层。这是 ADR 0012 的约束：包装层只做类型转换，不做语义决策。

## 深入了解

- [Import、Export 与主模块 ABI](../backend/imports-exports-and-abi.md)
- [ExecContext 与 Builtins 的解耦](exec-context-and-builtins.md)
- [核心 JavaScript Builtins 的分域组织](javascript-builtins.md)
