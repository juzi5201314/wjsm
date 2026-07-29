# 根、弱引用与 Finalizer

这一章说明 GC 如何找到根集，以及弱引用和 FinalizationRegistry 如何与 GC 协作。

## 根集

GC 的根集来自三个来源：

1. **栈上活跃句柄**：safepoint spill 写入影子栈的值（见[变量活跃性](../backend/liveness-slots-and-spills.md)）。
2. **RuntimeState 显式 root**：`roots.rs` 维护的一组 primordial 句柄（原型对象、全局构造器等）。
3. **Realm intrinsics**：每个 realm 的 `RealmIntrinsics` 结构里的原型句柄。

GC 从这三组根出发，遍历对象图，标记所有可达对象。

## 弱引用

`WeakRef` 持有对象的弱句柄。GC 回收时检查弱引用目标：如果目标不可达（不包含弱引用本身），目标被回收，`WeakRef.deref()` 返回 `undefined`。

WeakMap/WeakSet 的键是弱引用语义：键的可达性不包括 WeakMap 本身对它的引用。GC 回收键时自动清除对应条目。

## FinalizationRegistry

`FinalizationRegistry` 注册回调，在对象被 GC 回收时调用。回调是微任务，不是同步执行——GC 在 safepoint 发生，此时不能直接调用用户 JS。

回调的调度时机由运行时决定，不保证及时。如果进程在回调调度前退出，回调不会执行。

## 深入了解

- [safepoint spill 的后端实现](../backend/liveness-slots-and-spills.md)
- [RuntimeState 与 Realm 的 root 结构](../host-runtime/runtime-state-and-realms.md)
- [GC 何时触发与如何扫描](concurrency-and-pacing.md)
