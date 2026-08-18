# GC 选择、配置与不变量

这一章说明 GC 算法的选择逻辑、配置选项和必须遵守的不变量。

## 算法注册表

`GcAlgorithmKind` 枚举三种算法：`MarkSweep`、`G1`、`Zgc`。算法名是稳定字符串：`mark-sweep`、`g1`、`zgc`。`FromStr` 实现要求精确匹配，不接受大小写或分隔符变体。

生产默认是 `zgc`：并发分代 ZGC（`GenerationalZgc`）。`wjsm-host-native::NativeGc` 是唯一接合层——它拥有 `HeapAccessV2<NativeHeapMemory>`、mutator 与 collector 生命周期。`mark-sweep` / `g1` 走 `StopTheWorldCollector`；`zgc` 走 `GenerationalZgc`。

## 选择优先级

GC 算法的选择优先级（从高到低）：

1. `--gc` CLI 选项
2. `WJSM_TEST_GC` 环境变量（测试专用，覆盖普通配置）
3. `WJSM_GC` 环境变量
4. 默认值：`zgc`

`WJSM_TEST_GC` 优先级高于 `WJSM_GC`，让测试能强制指定 GC 而不受用户配置干扰。算法在 `NativeRuntime` 初始化后不可切换。

## 不变量

`crates/wjsm-gc/src/api.rs` 记录两条关键不变量：

- **INV-C1**：JS 值层引用是 handle；`obj_table[h]` 是唯一 ptr truth。
- **INV-C2**：raw ptr 不跨潜在 moving/collect GC 点；跨越必须重新 resolve。

这两条不变量适用于所有三种回收器。mark-sweep 虽然不移动对象，但代码仍遵守不变量，保持与移动回收器的兼容性。

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

- [用户侧的 GC 配置选项](../../user/configuration/gc.md)
- [Workspace crate 地图与依赖边界](../foundations/crate-map.md)
- [ADR 0010–0014 的决策记录](../reference/adr-index.md)
