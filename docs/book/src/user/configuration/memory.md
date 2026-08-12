# 堆、影子栈与内存预留

wjsm 进程的内存占用由三部分组成：JavaScript 托管堆、影子栈、native 镜像。前两者可配，后者固定。

## ManagedHeap 上限

`--max-heap-size <SIZE>` 限制 JavaScript 对象堆的分配预算。默认 64 MiB，支持 `K`/`M`/`G` 后缀。

```bash
wjsm --max-heap-size 256M run app.js
wjsm --max-heap-size 1G run app.js
```

预算耗尽时程序以运行时错误终止，不会继续增长内存。该上限与选哪个 GC 无关。

不能写进 `wjsm.toml`，只能用 CLI 或环境变量。

## GC 算法选择

`--gc <mark-sweep|g1|zgc>` 或 `WJSM_GC` 选择回收器，默认 `zgc`。详见 [垃圾回收器](gc.md)。

选择优先级：`--gc` > `WJSM_TEST_GC` > `WJSM_GC` > 默认 `zgc`。

GC 算法不影响堆上限，只影响回收策略和暂停时间：

| 值 | 暂停特性 | 适用场景 |
| --- | --- | --- |
| `mark-sweep` | STW，随堆增长 | 调试 GC 自身 |
| `g1` | STW 但分批，目标 200ms | 中等堆 |
| `zgc` | 并发，与堆大小基本无关 | 默认；大堆必须 |

## 影子栈（Shadow Stack）

影子栈是 GC root 扫描的内部机制，用户侧不可配置，但影响实际内存占用。

wjsm 的 GC 需要「在任意安全点能枚举所有栈上活跃的 JS 值引用」。由于 Cranelift codegen 不维护精确的栈映射，wjsm 在 safepoint 把活跃的句柄 spill 到一块预留区域——即影子栈。

影子栈的内存来自线性内存（linear memory）中预留的固定区域，大小在编译时确定，与程序调用深度相关。它不会随堆增长，但对深递归程序会有固定开销。

## 内存预留总量估算

实际进程内存 ≈ 对象堆 + 影子栈 + native 镜像 + 句柄表 + 页面元数据

| 部分 | 大小 | 说明 |
| --- | --- | --- |
| 对象堆 | `--max-heap-size` 设定 | 线性内存中的 memory64 区间 |
| 句柄表 | 随对象数线性增长 | 8 字节/句柄（V2） |
| 页面元数据 | 随堆页数增长 | mark bitmap、remset 等 |
| 影子栈 | 固定预留 | safepoint spill 区域 |
| native 镜像 | 固定 | 编译后的机器码 + 启动快照字节 |

对象堆的地址空间下限为 32 GiB（`HEAP_MEMORY_MIN_PAGES = 524288`），但实际占用由 `--max-heap-size` 控制。`--max-heap-size 256M` 不意味着进程只占 256 MiB——句柄表、元数据、影子栈和 native 镜像是额外的。

## 深入了解

- [垃圾回收器](gc.md)
- [ManagedHeap 架构](../../internals/gc/managed-heap.md)
- [Memory64 与共享内存模型](../../internals/gc/memory64.md)
- [GC 选择、配置与不变量](../../internals/gc/configuration-and-invariants.md)
