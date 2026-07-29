# 新增 Host Import

这一章说明如何向 WASM 模块添加新的 host import 函数。

## 何时需要新 host import

host import 是 user wasm 调用宿主函数的通道。需要新 import 的场景：

- 新的 builtin 需要调用宿主算法（见[新增 Builtin](adding-builtins.md)）。
- 新的 Node.js 模块需要宿主 I/O 能力。
- 新的运行时 API 暴露给 JS。

## 步骤

1. **规格定义**：在 `wjsm-backend-wasm/src/host_import_registry/` 的 `specs_part*.rs` 添加 import spec。spec 记录 import 名、type index 和特殊标记。
2. **codegen 引用**：user wasm 的 Pass 1 预留 import 索引，函数体编译时引用。
3. **host 函数实现**：在 `wjsm-host-wasm` 的对应 `runtime_*.rs` 文件实现 Rust 函数。函数从 `Caller<RuntimeState>` 取状态，调用 builtins 算法，返回 NaN-box 值。
4. **Linker 注册**：`runtime_linker.rs` 的 `register_host_imports` 或 `register_common_bridges` / `register_complex_bridges` 把函数注册到 Linker。
5. **ABI 哈希**：新 import 改变 support ABI，需要更新 `managed_heap_v2_support_abi_hash`。
6. **测试**：添加 fixture 或集成测试。

## 包装层的薄度

host import 函数大多是薄包装：从 `Caller` 取状态，调用 builtins 算法，编码返回值。语义逻辑在 builtins 层，不在包装层。这是 ADR 0012 的约束：包装层只做类型转换。

## ABI 影响

新增 host import 改变 support module 的 import 表，导致 `support_abi_union_hash` 变化，使 embedded startup snapshot 失配。build.rs 会重新生成 support cwasm 和 snapshot。这是预期行为，不是问题。

## 深入了解

- [Host Import 注册与包装层](../host-runtime/host-imports.md)
- [Import、Export 与主模块 ABI](../backend/imports-exports-and-abi.md)
- [修改 WASM ABI](changing-wasm-abi.md)
