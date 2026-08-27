# 垃圾回收器

wjsm 使用并发分代 ZGC（`GenerationalZgc`）作为唯一生产垃圾回收器。运行时不再提供 `--gc` 或 `WJSM_GC` 选择面；所有 agent 共用同一套 ManagedHeap 与 ZGC 屏障。

## 观察 GC 行为

当前没有面向用户的 `WJSM_GC_LOG`。比较回收性能时用 `--time`、`--stats` 和 `wjsm-gc-bench`。

## 与堆预算的关系

`--max-heap-size` 限制的是 JavaScript 堆的分配预算。预算耗尽时程序以运行时错误终止，而不是继续增长内存。

## 深入了解

- [ManagedHeap 架构](../../internals/gc/managed-heap.md)
- [Generational ZGC 的并发阶段与着色指针](../../internals/gc/zgc.md)
- [GC 配置与必须维持的不变量](../../internals/gc/configuration-and-invariants.md)
