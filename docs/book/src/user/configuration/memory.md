# 堆、root 帧与内存预留

wjsm 进程的内存占用由几部分组成：JavaScript 托管堆、GC root 帧、native 镜像和运行时表。用户能直接配置的是堆上限。生产 collector 固定为并发分代 ZGC，详见 [垃圾回收器](gc.md)。

## ManagedHeap 上限

`--max-heap-size <SIZE>` 限制 JavaScript 对象堆的分配预算。默认 64 MiB，支持 `K`/`M`/`G` 后缀。

```bash
wjsm --max-heap-size 256M run app.js
wjsm --max-heap-size 1G run app.js
```

预算耗尽时程序以运行时错误终止，不会继续增长内存。

不能写进 `wjsm.toml`，只能用 CLI。

## Root 帧

GC 需要在 safepoint 枚举栈上活跃的 JS 值引用。generated code 通过 `NativeRootFrame` 发布活跃槽位；collector 只扫描 bitmap 置位的 slot。这是内部机制，没有面向用户的尺寸开关。

深递归仍受调用深度与栈预算约束。

## 内存预留总量估算

实际进程内存 ≈ 对象堆已提交页 + 句柄表 + 页面元数据 + root 帧 + native 镜像 + 启动快照恢复后的 primordial 对象。

| 部分 | 大小 | 说明 |
| --- | --- | --- |
| 对象堆 | `--max-heap-size` 设定的分配预算 | `NativeHeapMemory` 按需提交 |
| 句柄表 | 随对象数线性增长 | 8 字节/句柄 |
| 页面元数据 | 随堆页数增长 | mark bitmap、remset 等 |
| native 镜像 | 随程序增长 | 当前宿主机器码；可选磁盘缓存 |

`--max-heap-size 256M` 不意味着进程只占 256 MiB。句柄表、元数据、native 镜像和嵌入快照都是额外占用。堆会按 64 KiB granule 提交物理页，不会因为逻辑地址空间更大就立刻占满。

## 深入了解

- [垃圾回收器](gc.md)
- [ManagedHeap 架构](../../internals/gc/managed-heap.md)
- [NativeHeapMemory 与逻辑堆地址](../../internals/gc/memory64.md)
- [GC 选择、配置与不变量](../../internals/gc/configuration-and-invariants.md)
