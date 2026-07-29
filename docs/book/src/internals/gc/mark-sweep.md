# Mark-Sweep

mark-sweep 是三种回收器里实现最简单的，行为最容易预测。这一章说明它的工作方式。

## 算法

1. **标记**：从根集出发，遍历对象图，标记所有可达对象。mark bitmap 每页一位，标记通过 `mark_bitmap.rs` 完成。
2. **清除**：遍历堆，回收未标记的对象。回收的内存加入空闲列表。

两个阶段都是 STW（stop-the-world），没有并发。这意味着回收期间程序完全暂停。

## 何时触发

`__gc_alloc_bytes` 记录自上次 GC 以来的分配量，`__gc_trigger_bytes` 是触发阈值。分配 fast path 每次推进 `__alloc_ptr` 时累加，达到阈值时触发 GC。

## 适用场景

mark-sweep 的暂停时间与堆大小成正比，大堆上暂停明显。它适合：

- 小堆程序。
- 调试时需要可预测的 GC 行为。
- 作为其他回收器的参考实现。

## 碎片

mark-sweep 不移动对象，长期运行会产生碎片。空闲列表合并相邻块缓解碎片，但不消除。`GcStats` 的 `external_fragmentation` 指标反映碎片程度。

## 深入了解

- [G1 如何通过分区回收治理碎片](g1.md)
- [ZGC 如何通过并发移动消除碎片](zgc.md)
- [GC 选择逻辑](configuration-and-invariants.md)
