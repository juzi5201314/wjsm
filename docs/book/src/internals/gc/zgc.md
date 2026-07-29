# Generational ZGC

ZGC 是并发移动回收器，暂停时间与堆大小无关。这一章说明它的核心机制。

## 着色指针

ZGC 的核心是着色指针（colored pointer）。对象指针的高位编码 GC 状态：

- `Marked0` / `Marked1`：标记位，交替使用。
- `Remapped`：指针指向最新地址。
- `Finalizable`：对象在本次 GC 中首次标记。

读屏障检查指针颜色，如果指针过时（指向旧地址），转发到新地址并更新。这让对象移动可以并发进行——读者通过屏障自动转发，不需要 STW。

## 并发阶段

ZGC 的 GC 周期分为几个阶段，大部分并发执行：

1. **初始标记**：STW，从根集出发标记直接可达对象。
2. **并发标记**：工作线程并发遍历对象图，标记所有可达对象。
3. **再标记**：STW，处理标记结束时的 SATB 缓冲区。
4. **并发转移**：工作线程并发移动对象，更新指针。
5. **再映射**：并发修复过时指针。

STW 阶段只处理根集，暂停时间与根集大小相关，与堆大小无关。

## 分代

Generational ZGC 把 young 对象和 old 对象分开管理。young GC 只扫 young 区，频率高、成本低。old GC 处理整个堆，频率低。

分代引入了跨代引用问题，通过写屏障和 remset 解决（见[屏障与 remset](barriers-and-remset.md)）。

## 默认回收器

ZGC 是 wjsm 的默认 GC 算法。`--gc` 选项、`WJSM_GC` 环境变量可以切换到 mark-sweep 或 G1。选择逻辑见[GC 选择与配置](configuration-and-invariants.md)。

## 深入了解

- [着色指针与读屏障的配合](barriers-and-remset.md)
- [G1 的分区回收对比](g1.md)
- [用户侧的 GC 配置](../../user/configuration/gc.md)
