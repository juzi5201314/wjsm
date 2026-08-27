# 修改 GC 与堆布局

这一章说明修改 GC 算法或堆布局时需要改动哪些地方。

## 不变量

`wjsm-gc/src/api.rs` 记录两条关键不变量：

- **INV-C1**：JS 值层引用是 handle；`obj_table[h]` 是唯一 ptr truth。
- **INV-C2**：raw ptr 不跨潜在 moving/collect GC 点；跨越必须重新 resolve。

修改 GC 必须保持这两条不变量。

## 改动步骤

1. **GC 算法**：`wjsm-gc/src/heap/` 的对应文件（`allocator.rs`、`bitmap.rs`、`page.rs` 等）修改 `GenerationalZgc` 策略。
2. **屏障**：更新 `wjsm-gc` 屏障与 `NativeBarrierState` / `NativeHostSymbol` 叶子 thunk。
3. **接合层**：`wjsm-host-native::NativeGc` 绑定 `GenerationalZgc` 与 `NativeHeapMemory`。
4. **Native ABI**：vmctx 上的 `heap_state` / `gc_state` / `barrier_state` / `heap_object_delta` 若变化，走 `native_abi_hash()`。
5. **测试**：`crates/wjsm-gc/` 的单元测试，`gc-benchmarks` 的基准测试，fixture 验证行为。

## 统一 ManagedHeap

ADR 0010 确立统一 ManagedHeap。不能引入 memory32 对象堆、4 字节句柄、dual-heap fallback 或第二个运行时 owner。所有修改必须基于 8 字节句柄和 `NativeHeapMemory` 的 memory64 逻辑偏移。

## 深入了解

- [ManagedHeap 架构](../gc/managed-heap.md)
- [GC 选择、配置与不变量](../gc/configuration-and-invariants.md)
- [修改 Native ABI](changing-wasm-abi.md)
