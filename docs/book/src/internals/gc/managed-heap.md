# ManagedHeap 架构

这一章说明统一托管堆的组织，以及三种回收器如何共用它。

## 统一路径

ADR 0010 确立了统一 ManagedHeap：mark-sweep、G1、ZGC 三种回收器都跑在 shared memory64 对象堆上，使用 8 字节句柄。旧设计的 memory32 对象堆、4 字节句柄和 dual-heap fallback 已完全移除。

## 堆的组成

ManagedHeap 由几个部分组成：

- **对象堆**：shared memory64 线性内存，对象数据按布局分配。
- **Handle Table**（`HandleTableV2`）：句柄到堆指针的映射表，`obj_table[handle] → heap_ptr`。
- **页面元数据**：每页的 mark bitmap、remset 等元数据。
- **分配器**：bump pointer 分配 + 空闲列表。

三种回收器共享这三部分，区别在于扫描、标记和清除的算法不同。

## 句柄 vs 指针

JavaScript 值持有的是句柄（`Handle = u32`），不是裸指针。句柄是对象表的下标，GC 可以移动对象而不需要更新值——只更新表里的指针。

这条不变量（`INV-C1`）是整个设计的基础。raw pointer 不跨潜在 moving/collect GC 点（`INV-C2`），跨越时必须重新 resolve。

## 深入了解

- [Memory64 与共享内存模型](memory64.md)
- [Handle Table 的结构与重映射](handle-table.md)
- [对象布局与分配](object-layout-and-allocation.md)
- [ADR 0010 的决策记录](../reference/adr-index.md)
