# 修改 GC 与堆布局

这一章说明修改 GC 算法或堆布局时需要改动哪些地方。

## 不变量

`wjsm-gc/src/api.rs` 记录两条关键不变量：

- **INV-C1**：JS 值层引用是 handle；`obj_table[h]` 是唯一 ptr truth。
- **INV-C2**：raw ptr 不跨潜在 moving/collect GC 点；跨越必须重新 resolve。

修改 GC 必须保持这两条不变量。mark-sweep 虽然不移动对象，但代码仍遵守不变量。

## 改动步骤

1. **GC 算法**：`wjsm-gc/src/heap/` 的对应文件（`allocator.rs`、`bitmap.rs`、`page.rs` 等）修改算法。
2. **屏障**：如果新算法需要不同的屏障，`barriers.rs` 或 `wjsm-host-native` 的屏障调用更新。
3. **注册表**：`wjsm-gc/src/registry.rs` 如果新增算法，`GcAlgorithmKind` 添加 variant，`FromStr` 和 `as_str` 更新。
4. **Native ABI**：`wjsm-native-abi` 为新 flavor 更新 GC 相关的 vmctx 布局（如果 env global 变化）。
5. **build.rs**：如果新增 flavor，更新构建配置。
6. **vmctx**：如果 env global 变化（如新的 GC 状态 global），`wjsm-native-abi` 和 `wjsm-host-native` 的 vmctx 初始化更新。
7. **测试**：`crates/wjsm-gc/` 的单元测试，`gc-benchmarks` 的基准测试，fixture 验证行为。

## 统一 ManagedHeap

ADR 0010 确立统一 ManagedHeap。不能引入 memory32 对象堆、4 字节句柄、dual-heap fallback 或第二个运行时 owner。所有修改必须基于 8 字节句柄和 shared memory64。

## 深入了解

- [ManagedHeap 架构](../gc/managed-heap.md)
- [GC 选择、配置与不变量](../gc/configuration-and-invariants.md)
- [修改 WASM ABI](changing-wasm-abi.md)
