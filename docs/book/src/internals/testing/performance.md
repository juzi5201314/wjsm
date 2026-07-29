# 性能分析与回归证据

这一章说明 wjsm 的性能分析和回归检测方法。

## 性能维度

| 维度 | 测量方法 | 关注点 |
| --- | --- | --- |
| 启动时间 | 进程启动到用户代码开始执行 | 冷启动、快照命中率 |
| 编译时间 | `compile_source` 耗时 | Cranelift 编译、缓存命中率 |
| 执行时间 | 用户代码执行耗时 | 代码生成质量 |
| GC 暂停 | GcStats 的 `elapsed` | 回收器选择 |
| 内存占用 | 堆使用量、对象表大小 | 内存泄漏、碎片 |

## runtime_bench

`crates/wjsm-host-wasm/src/runtime_bench.rs` 提供基准框架。它利用 `embedded_startup_snapshot_view` 等 API 确保基准测试在一致的状态下运行。

## WASMTIME_VERSION

`WASMTIME_VERSION = "43.0.2"` 是 engine owner 绑定的精确版本，用于 benchmark evidence。版本变化时 benchmark 基线需要重新建立。

## 回归检测

性能回归通过对比基线检测：

- 启动时间：新改动不应让冷启动或快照恢复变慢。
- 编译时间：codegen 改动可能影响编译速度。
- 执行时间：lowering 或 codegen 质量回归。
- GC：算法改动影响暂停时间。

回归超过阈值时，需要分析原因并优化或回滚。

## Cranelift vs Winch

`WJSM_COMPILER` 切换编译器。Cranelift 优化执行速度但编译慢，Winch 编译快但执行慢（基线 JIT）。benchmark 需要在两种 compiler 下都跑，评估各自的 trade-off。

## 深入了解

- [GC Benchmark](gc-benchmarks.md)
- [Engine 配置与池化](../startup/engine-pool.md)
- [用户侧的性能与内存调优](../../user/workflows/performance.md)
