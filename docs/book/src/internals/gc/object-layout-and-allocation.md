# 对象布局与分配

这一章说明对象在堆中的内存布局和分配路径。

## 堆组织

`crates/wjsm-gc/src/heap/` 包含堆的核心组件：

| 文件 | 职责 |
| --- | --- |
| `allocator.rs` | bump pointer 分配 + 空闲列表 |
| `bitmap.rs` | mark bitmap，每页一位 |
| `page.rs` | 页元数据 |
| `layout.rs` | 对象布局常量 |
| `handle.rs` / `handle_entry.rs` | 句柄表项 |
| `object_map.rs` | 对象起始地址映射 |
| `memory.rs` / `native_memory.rs` | 内存抽象 |
| `epoch.rs` | 分配 epoch（ZGC 着色指针） |
| `word.rs` | 字长常量 |

## 分配路径

1. **fast path**：bump pointer，`alloc_ptr` 前进。绝大多数分配走这条路径。
2. **slow path**：bump 到页边界时调用 `gc_alloc_slow` host 函数，触发 GC 或扩展内存。

`__alloc_ptr` 和 `__alloc_end` 是 env global，后端直接读写。fast path 完全在 WASM 内完成，不跨宿主调用。

## 对象布局

每个对象在堆中的布局：

- 类型标记（HeapType）：Object、Array、Promise、Continuation、Map、Set 等。
- 属性存储：内联槽或外部属性表。
- 内部槽：`[[InternalSlot]]` 存为固定 offset 的内联字段，GC 追踪。

类型标记决定 GC 如何扫描对象内部引用。例如 Array 对象的元素区可能包含句柄，需要逐个扫描。

## 碎片治理

`GcStats` 记录碎片指标：`free_block_count`、`total_free_bytes`、`largest_free_block`、`external_fragmentation`。G1 回收器按分区回收治理碎片，mark-sweep 通过空闲列表合并相邻块。

## 深入了解

- [G1 的分区与回收集选择](g1.md)
- [Mark-Sweep 的标记与清除阶段](mark-sweep.md)
- [GC 统计与碎片指标](configuration-and-invariants.md)
