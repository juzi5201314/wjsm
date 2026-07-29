# 堆、影子栈与内存预留

wjsm 有三块独立的内存区域，各自有独立的上限选项。调优时先确认问题出在哪一块，再动对应选项。

| 区域 | 选项 | 默认 |
| --- | --- | --- |
| JavaScript 堆 | `--max-heap-size <SIZE>` | 不限制 |
| 影子栈 | `--shadow-stack-max <SIZE>` 或 `WJSM_SHADOW_STACK_MAX` | 16 MiB |
| Wasmtime 线性内存虚拟预留 | `--wasmtime-memory-reservation <SIZE>` | 由 Wasmtime 决定 |

`SIZE` 接受纯字节数或 `K`/`M`/`G` 后缀（也接受 `KB`、`MiB` 等写法，均按 1024 进制换算）。`0` 和未知后缀会被拒绝：

```text
error: invalid value '0' for '--max-heap-size <SIZE>': heap size must be greater than zero
error: invalid value '10T' for '--max-heap-size <SIZE>': unsupported heap size suffix `T`
```

## JavaScript 堆预算

`--max-heap-size` 给对象分配设定硬预算。超出后程序终止并报告已用量：

```bash
wjsm --max-heap-size 8M run app.js
```

```text
Runtime error: JavaScript heap budget exhausted: requested 144 bytes with 8388608/8388608 bytes used
```

这是预算耗尽，不是 GC 失效。要判断是真的需要更多内存还是存在对象泄漏，配合 `WJSM_GC_LOG=1` 观察回收后存活量的走势。

## 影子栈

影子栈是与对象堆分离的一块线性内存，承载变长参数传递和函数调用期间需要保留的值。它冷启动只占 64 KiB，按需增长，触到软上限时抛出 `RangeError`。

深递归程序如果报影子栈溢出，提高上限：

```bash
wjsm --shadow-stack-max 64M run deep.js
WJSM_SHADOW_STACK_MAX=64M wjsm run deep.js
```

`--shadow-stack-max` 优先于 `WJSM_SHADOW_STACK_MAX`；两者都没给时使用 16 MiB。

## Wasmtime 内存预留

`--wasmtime-memory-reservation` 调整 Wasmtime 为线性内存预留的**虚拟**地址空间，不改变实际占用的物理内存。在虚拟地址紧张的环境（容器限制、32 位地址空间、并发跑很多实例）调小它可以避免映射失败。

## 深入了解

- [ManagedHeap 如何在共享 memory64 上组织对象](../../internals/gc/managed-heap.md)
- [Memory64 与共享内存模型](../../internals/gc/memory64.md)
- [影子栈槽位活跃性与 GC Spill 规则](../../internals/backend/liveness-slots-and-spills.md)
- [Engine 配置与线性内存预留参数](../../internals/host-runtime/engine-configuration.md)
