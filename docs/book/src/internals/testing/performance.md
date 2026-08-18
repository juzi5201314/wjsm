# 性能分析与回归证据

这一章说明 wjsm 的性能分析和回归检测方法。

## 性能维度

| 维度 | 测量方法 | 关注点 |
| --- | --- | --- |
| 启动时间 | 进程启动到用户代码开始执行 | 冷启动、快照恢复、磁盘缓存 |
| 编译时间 | IR → CLIF → native image | Cranelift 编译、缓存命中率 |
| 执行时间 | 用户代码执行耗时 | 代码生成质量 |
| GC 暂停 | GcStats 的 `elapsed` | 回收器选择 |
| 内存占用 | 堆使用量、对象表大小 | 内存泄漏、碎片 |

## runtime_bench

`crates/wjsm-host-native/src/runtime_bench.rs` 提供基准框架。它在一致的 `NativeRuntime` 状态下跑微基准。

## 回归检测

性能回归通过对比基线检测：

- 启动时间：新改动不应让冷启动或快照恢复变慢。
- 编译时间：codegen 改动可能影响 IR → CLIF → native 的速度。
- 执行时间：lowering 或 codegen 质量回归。
- GC：算法改动影响暂停时间。

回归超过阈值时，需要分析原因并优化或回滚。

## 编译器

生产路径只有 Direct Cranelift（`wjsm-backend-native`）。没有 Winch，也没有 `WJSM_COMPILER` 开关。`WJSM_OPT_LEVEL`（`none` / `speed` / `speed_and_size`，未设置即 `speed`）只改变 Cranelift 优化档，并进入 native cache 键。

## 深入了解

- [GC Benchmark](gc-benchmarks.md)
- [跨运行时基准](cross-runtime-benchmarks.md)
- [Engine 配置与池化](../startup/engine-pool.md)
- [用户侧的性能与内存调优](../../user/workflows/performance.md)
