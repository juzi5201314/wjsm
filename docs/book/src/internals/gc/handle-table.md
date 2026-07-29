# Handle Table

`HandleTableV2` 是句柄到堆指针的映射表。这一章说明它的结构与生命周期。

## 结构

```
obj_table[handle] → heap_ptr
```

句柄（`Handle = u32`）是表的下标，表项存堆指针。JavaScript 值通过 `TAG_OBJECT_HANDLE` 标签加句柄负载引用对象。

GC 移动对象时只更新表项，不需要扫描所有 NaN-box 值。这是句柄间接层的主要收益。

## 分配与回收

分配对象时：

1. 从 free handle list 取一个句柄（或扩展表）。
2. 在对象堆分配内存，得到堆指针。
3. `obj_table[handle] = heap_ptr`。

GC 回收对象时：

1. 标记阶段从根集出发，标记所有可达句柄。
2. 清除阶段释放未标记句柄对应的堆内存。
3. 把回收的句柄放回 free handle list。

`gc_take_freed_handle` 是 support module import 的 host 函数，从 free list 取句柄。

## 8 字节句柄

ADR 0010 统一为 8 字节句柄（V2）。旧的 4 字节句柄（V1）已完全移除。8 字节句柄的负载放在 NaN-box 值的低 32 位，与 V1 兼容的编码方式一致，但表项宽度不同。

## 深入了解

- [ManagedHeap 架构](managed-heap.md)
- [NaN-boxed 值表示中的 TAG_OBJECT_HANDLE](../backend/value-representation.md)
- [对象布局与分配](object-layout-and-allocation.md)
