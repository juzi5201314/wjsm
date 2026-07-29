# GC Benchmark

这一章说明 GC 性能基准测试。

## 目标

GC benchmark 测量三种回收器（mark-sweep、g1、zgc）在不同负载下的性能：

- 暂停时间分布。
- 吞吐量（每秒分配对象数）。
- 内存占用。
- 碎片程度。

## 运行方式

```bash
cargo nextest run -E 'test(gc_benchmark)'
```

benchmark 通过 `WJSM_TEST_GC` 环境变量切换 GC 算法，在相同负载下比较。

## GcStats

`GcStats` 记录每次 GC 周期的统计，benchmark 汇总这些数据：

| 指标 | 含义 |
| --- | --- |
| `marked` | 标记的对象数 |
| `swept` | 清除的对象数 |
| `freed_bytes` | 释放的字节数 |
| `elapsed` | 本次周期耗时 |
| `external_fragmentation` | 外部碎片率 |

benchmark 报告这些指标的分布（min / p50 / p99 / max），帮助评估 GC 对延迟敏感场景的适用性。

## 回归检测

benchmark 结果作为回归基线。后续改动如果让 GC 性能下降（暂停时间增加、吞吐量降低），benchmark 能捕获。`runtime_bench.rs` 提供基准框架。

## 深入了解

- [GC 选择、配置与不变量](../gc/configuration-and-invariants.md)
- [性能分析与回归证据](performance.md)
- [用户侧的性能与内存调优](../../user/workflows/performance.md)
