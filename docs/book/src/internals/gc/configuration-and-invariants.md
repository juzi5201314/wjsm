# GC 配置与不变量

这一章说明生产 GC 的配置边界和必须遵守的不变量。

## 生产 collector

运行时固定使用并发分代 ZGC（`GenerationalZgc`）。`wjsm-host-native::NativeGc` 是唯一接合层——它拥有 `HeapAccessV2<NativeHeapMemory>`、mutator 与 collector 生命周期，并始终安装 `ZgcBarrierSet`。

## 不变量

`crates/wjsm-gc/src/api.rs` 记录两条关键不变量：

- **INV-C1**：JS 值层引用是 handle；`obj_table[h]` 是唯一 ptr truth。
- **INV-C2**：raw ptr 不跨潜在 moving/collect GC 点；跨越必须重新 resolve。

## 统计

`GcStats` 记录每次 GC 周期的统计：

| 字段 | 含义 |
| --- | --- |
| `marked` | 标记的对象数 |
| `swept` | 清除的对象数 |
| `freed_bytes` | 释放的字节数 |
| `elapsed` | 本次周期耗时 |
| `free_block_count` | 空闲块总数 |
| `total_free_bytes` | 总空闲字节 |
| `largest_free_block` | 最大连续空闲块 |
| `external_fragmentation` | 外部碎片率 |

`CycleKind` 区分周期类型：`Full`、`Young`、`Mixed`、`ZgcCycle`、`Step`。

## 后端无关

GC 算法实现在 `wjsm-gc` crate，不依赖 Cranelift。`wjsm-host-native` 的 `NativeGc` 把 `NativeHeapMemory`、`NativeRootFrame` 扫描和屏障状态接到 `NativeVmContext`。`wjsm-builtins`、`wjsm-host`、`wjsm-module` 不拥有堆。这是 ADR 0011–0014 的边界：Cranelift / 平台依赖只留在 `wjsm-backend-native` 与 `wjsm-host-native`。

## 深入了解

- [用户侧的 GC 说明](../../user/configuration/gc.md)
- [Workspace crate 地图与依赖边界](../foundations/crate-map.md)
- [ADR 0010–0014 的决策记录](../reference/adr-index.md)
