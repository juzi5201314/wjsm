# Handle Table

`HandleTableV2` 是句柄到堆指针的映射表。句柄（`Handle = u32`）是表的下标，表项存堆指针。JavaScript 值通过 `TAG_OBJECT_HANDLE` 标签加句柄负载引用对象。

## 结构

```mermaid
flowchart LR
    subgraph Value[Nan-box 值]
        V1[标签 0x0000_003F_0000_0000<br/>TAG_OBJECT_HANDLE = 6] --> V2[负载: Handle = u32<br/>句柄下标]
    end
    subgraph Table[HandleTableV2]
        T1[obj_table[0]] --> P1[heap_ptr 0x...]
        T2[obj_table[1]] --> P2[heap_ptr 0x...]
        T3[obj_table[2]] --> P3[heap_ptr 0x...]
        TN[...] --> PN[...]
    end
    V2 --> T1
```

GC 移动对象时只更新表项，不需要扫描所有 NaN-box 值。这是句柄间接层的主要收益。

## 分配与回收

| 阶段 | 操作 |
| --- | --- |
| 分配对象 | 从 free handle list 取句柄 → 堆分配 → `obj_table[handle] = heap_ptr` |
| 标记阶段 | 从根集出发，标记所有可达句柄 |
| 清除阶段 | 释放未标记句柄对应的堆内存，句柄放回 free list |

`gc_take_freed_handle` 是 support module import 的 host 函数，从 free list 取句柄。

## 8 字节句柄

ADR 0010 统一为 8 字节句柄（V2）。旧的 4 字节句柄（V1）已完全移除。8 字节句柄的负载放在 NaN-box 值的低 32 位，与 V1 兼容的编码方式一致，但表项宽度不同。

## 深入了解

- [ManagedHeap 架构](managed-heap.md)
- [NaN-boxed 值表示中的 TAG_OBJECT_HANDLE](../backend/value-representation.md)
- [对象布局与分配](object-layout-and-allocation.md)
