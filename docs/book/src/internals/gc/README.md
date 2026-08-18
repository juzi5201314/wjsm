# GC 算法、对象模型与栈布局

这一部分说明统一 ManagedHeap、句柄表，以及 mark-sweep / G1 / ZGC 如何共用同一套堆。

## ManagedHeap 与 HandleTableV2

`wjsm-gc` 的 `ManagedHeap` 是对象堆 owner。生产路径用 `NativeHeapMemory`：mmap 保留逻辑容量，按 64 KiB granule 提交物理页。JS 值持有 handle，不持有可跨 safepoint 的裸指针。

`HandleTableV2` 把 `Handle` 映射到逻辑堆地址。GC 移动对象时只改表项。

## mark-sweep | G1 | ZGC

三种算法分别在 `mark_sweep/`、`g1/`、`zgc/`。宿主通过 `GcAlgorithmKind` 选择，由 `wjsm-host-native::NativeGc` 接到同一 `HeapAccessV2<NativeHeapMemory>`。

- **mark-sweep**：标记-清除，行为最可预测。
- **G1**：分区回收，region remset。
- **ZGC**（默认）：着色指针、分代并发标记与转移。

> <details><summary>为什么句柄不是裸指针？</summary>
>
> 裸指针不能安全跨越移动式 GC。句柄是表下标，GC 更新表项后旧 handle 仍然有效。
>
> </details>

## Root 帧

may-GC 点由 generated code 发布 `NativeRootFrame`（`slots` + bitmap）。collector 只扫描 bitmap 置位的槽。没有独立的影子栈线性内存，也没有 Wasm 导入的 shadow memory。

## 与 builtins、host runtime 关系

GC 算法只经 `HeapMemory` / `GrowableHeapMemory` 访问堆，不绑定 Cranelift。`wjsm-builtins` 继续走 `ExecContext`，不直接操作 mmap。

## 深入了解

- [NativeHeapMemory 与逻辑堆地址](memory64.md)
- [活跃性、槽位与 GC Spill](../backend/liveness-slots-and-spills.md)
- [对象布局与分配](object-layout-and-allocation.md)
- [GC 选择、配置与不变量](configuration-and-invariants.md)
