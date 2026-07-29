# 修改 GC 与堆布局

这一章说明修改 GC 算法或堆布局时需要改动哪些地方。

## 不变量

`wjsm-gc/src/api.rs` 记录两条关键不变量：

- **INV-C1**：JS 值层引用是 handle；`obj_table[h]` 是唯一 ptr truth。
- **INV-C2**：raw ptr 不跨潜在 moving/collect GC 点；跨越必须重新 resolve。

修改 GC 必须保持这两条不变量。mark-sweep 虽然不移动对象，但代码仍遵守不变量。

## 改动步骤

1. **GC 算法**：`wjsm-gc/src/heap/` 的对应文件（`allocator.rs`、`bitmap.rs`、`page.rs` 等）修改算法。
2. **屏障**：如果新算法需要不同的屏障，`barriers.rs`（如果有）或 host-wasm 的屏障调用更新。
3. **注册表**：`wjsm-gc/src/registry.rs` 如果新增算法，`GcAlgorithmKind` 添加 variant，`FromStr` 和 `as_str` 更新。
4. **support module**：`wjsm-backend-wasm/src/support_module.rs` 为新 flavor 添加 support 函数。如果是修改现有算法，更新对应 flavor 的 support。
5. **build.rs**：如果新增 flavor，`build.rs` 的 flavor 列表添加新 flavor，生成对应 cwasm。
6. **WasmEnv**：如果 env global 变化（如新的 GC 状态 global），`wasm_env.rs` 和 `extract_wasm_env` 更新。
7. **测试**：`crates/wjsm-gc/` 的单元测试，`gc-benchmarks` 的基准测试，fixture 验证行为。

## 统一 ManagedHeap

ADR 0010 确立统一 ManagedHeap。不能引入 memory32 对象堆、4 字节句柄、dual-heap fallback 或第二个运行时 owner。所有修改必须基于 8 字节句柄和 shared memory64。

## 深入了解

- [ManagedHeap 架构](../gc/managed-heap.md)
- [GC 选择、配置与不变量](../gc/configuration-and-invariants.md)
- [修改 WASM ABI](changing-wasm-abi.md)
