# 写屏障、读屏障与 Remset

这一章说明 GC 屏障如何维护跨代/跨分区引用，以及 remset 如何记录它们。

## 为什么需要屏障

分代 GC 的基本假设是「young 对象很少引用 old 对象」。如果 old 对象引用 young 对象，young GC 只扫 young 区会漏掉这个引用。屏障在写入时记录跨代引用，让 young GC 能找到它。

ZGC 使用着色指针，屏障在读取时检查指针颜色，必要时转发到新地址。这避免了 STW 的对象移动。

## 写屏障

写屏障在属性赋值（`obj_set`、`elem_set`）时触发。如果写入的值是句柄，且写入方与被写入方在不同代/分区，记录到 remset。

`__good_color` 和 `__barrier_buf_ptr` / `__barrier_buf_end` 是 ZGC 写屏障使用的 env global。屏障缓冲区满时调用宿主函数刷新。

## 读屏障

ZGC 的读屏障在读取对象字段时检查指针颜色。着色指针在高位编码 epoch 信息，读屏障据此判断指针是否需要转发。

读屏障的开销高于写屏障（读比写频繁），但 ZGC 通过它在并发移动对象时保持正确性。

## Remset

remset（remembered set）记录「哪些 old 对象引用了 young 对象」。young GC 扫描 remset 里的 old 对象，而不是全部 old 区。

G1 的 remset 是分区粒度的：每个分区维护一个「引用本分区 young 对象的 old 分区」集合。ZGC 不使用传统 remset，改用着色指针和并发标记。

## 深入了解

- [G1 的分区与回收集选择](g1.md)
- [ZGC 的着色指针与并发移动](zgc.md)
- [GC 不变量](configuration-and-invariants.md)
