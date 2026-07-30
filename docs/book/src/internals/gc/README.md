# GC 算法、对象模型与栈布局

这一章涵盖 ManagedHeap、句柄表、GC 算法（mark-sweep、G1、Zgc），平衡性能与安全的设计与所有权边界。

## ManagedHeap 与 HandleTableV2

`wjsm-gc` 的 `ManagedHeap<T>` 是 64 位对象堆的 owner，内部以 `Vec<u8>` 管理分配，句柄表`HandleTableV2` 按对象类型划分 slot。

句柄是 table index，不是指针——GC 移动对象只需更新表，所有值持 handle 的 i64 不动。

## mark-sweep | G1 | Zgc

三种 GC 算法实现于 `gc_mark_sweep.rs`, `gc_g1.rs`, `gc_zgc.rs`。对外暴露 12 个核心接口（对象分配、回收、屏障、并发/暂停、统计）。宿主通过 trait `GcFlavor` 选择算法，test 确认三套行为都能无缝替换。

- **mark-sweep**：极简标记清扫，适合小型实例。
- **G1**：分区并发收集，region 粒度 remset。
- **Zgc**：着色指针、并发标记与代际写屏障。

> <details><summary>为什么句柄不是裸指针？</summary>
>
> 裸指针不能安全移动对象（GC 移动会失效），句柄 index 可以被 GC 重分配，保持引用一致。
>
> </details>

## Shadow Stack 与 spill

GC spill 路径见 backend/liveness-slots-and-spills，影子栈作为 safepoint spill zone，挂在主线性内存或 shadow memory，支持多实例隔离。

异常边界、梯度并发、跨实例引用都通过 shadow stack 管理。

## 与 builtins、host runtime 关系

GC 接口与 builtins 统一契约：单态化实现 `GrowableHeapMemory` trait，不绑定具体后端，所有内置能力可无缝复用。

## 深入了解

- [影子栈溢出与 GC spill](../backend/liveness-slots-and-spills.md)
- [对象布局与分配相关的 GC 细节](object-layout-and-allocation.md)
- [多 GC flavor 切换与隔离策略](../startup/embedded-artifacts.md)
