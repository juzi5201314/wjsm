# 托管堆与垃圾回收

这一部分讲 wjsm 的对象堆与三种垃圾回收器。

三种回收器共用同一个 ManagedHeap，跑在 shared memory64 上。ADR 0010 取代了旧的 pluggable GC v2（memory32、4 字节句柄）设计，统一为 8 字节句柄和 memory64 对象堆。

- [ManagedHeap 架构](managed-heap.md)
- [Memory64 与共享内存模型](memory64.md)
- [Handle Table](handle-table.md)
- [对象布局与分配](object-layout-and-allocation.md)
- [根、弱引用与 Finalizer](roots-weak-and-finalizers.md)
- [写屏障、读屏障与 Remset](barriers-and-remset.md)
- [Mark-Sweep](mark-sweep.md)
- [G1](g1.md)
- [Generational ZGC](zgc.md)
- [并发阶段、工作线程与 Pacing](concurrency-and-pacing.md)
- [GC 选择、配置与不变量](configuration-and-invariants.md)
