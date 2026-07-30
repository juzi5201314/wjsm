# Mark-Sweep

mark-sweep 是三种回收器里实现最简单的，行为最容易预测。两个阶段都是 STW（stop-the-world），没有并发，回收期间程序完全暂停。

## 算法

```mermaid
flowchart LR
    Roots[根集] --> Mark[标记阶段<br/>遍历对象图] --> Bitmap[mark bitmap 标记可达]
    Bitmap --> Sweep[清除阶段<br/>遍历堆，回收未标记对象] --> FreeList[空闲列表]
    FreeList --> Alloc[新分配使用空闲列表]
```

1. **标记**：从根集出发，遍历对象图，在 mark bitmap 中标记所有可达对象。`crates/wjsm-gc/src/heap/bitmap.rs` 实现 bitmap 操作，每页一位。
2. **清除**：遍历堆，回收未标记的对象。回收的内存加入空闲列表，按块大小排序，下次分配优先使用。

## 触发条件

`__gc_alloc_bytes` 记录自上次 GC 以来的分配量，`__gc_trigger_bytes` 是触发阈值。分配 fast path 每次推进 `__alloc_ptr` 时累加，达到阈值时触发 GC。

## 适用场景

| 场景 | 是否推荐 | 原因 |
| --- | --- | --- |
| 小堆程序（< 64 MiB） | 推荐 | 标记-清除的开销与堆大小成正比，小堆上暂停可接受 |
| 调试与行为验证 | 推荐 | 无并发、无移动，行为最可预测 |
| 大堆（> 256 MiB） | 不推荐 | 暂停时间与堆大小成正比，大堆上暂停明显 |
| 低延迟应用 | 不推荐 | 没有并发阶段，STW 时间不可控 |

## 碎片

mark-sweep 不移动对象，长期运行会产生碎片。空闲列表合并相邻块缓解碎片，但不消除。`GcStats` 的 `external_fragmentation` 指标反映碎片程度。当碎片率超过阈值时，`GcStats::largest_free_block` 会显著小于 `total_free_bytes`。

## 参考实现价值

mark-sweep 作为其他回收器的参考实现：字面量最小、不变量最清晰。G1 和 ZGC 的标记阶段共用 mark bitmap 基础设施，但清除和碎片管理方式不同。

## 深入了解

- [G1 如何通过 region 回收治理碎片](g1.md)
- [ZGC 如何通过并发移动消除碎片](zgc.md)
- [GC 选择逻辑](configuration-and-invariants.md)
