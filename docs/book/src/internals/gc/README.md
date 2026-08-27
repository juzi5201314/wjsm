# GC 算法、对象模型与栈布局

这一部分说明统一 ManagedHeap、句柄表，以及 Generational ZGC 如何管理对象堆。

## Generational ZGC

生产路径由 `wjsm-gc::GenerationalZgc` 实现，经 `wjsm-host-native::NativeGc` 接到同一 `HeapAccessV2<NativeHeapMemory>`。并发 mark/relocate、分代与 epoch reclaim 由 ZGC 屏障与 worker pool 驱动。

## 深入了解

- [ManagedHeap 架构](managed-heap.md)
- [Generational ZGC](zgc.md)
- [写屏障、读屏障与 Remset](barriers-and-remset.md)
- [GC 配置与不变量](configuration-and-invariants.md)
