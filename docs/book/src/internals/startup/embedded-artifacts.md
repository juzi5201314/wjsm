# 嵌入工件与多 GC flavor 切换

这一章说明 startup snapshot 如何影响 GC flavor 选择，以及嵌入工件的管理。

## 启动快照

startup snapshot 是 bootstrap 后的堆状态序列化。它保存 primordial 对象（`Object.prototype`、`Array.prototype` 等）和全局构造器的状态。

| 模式 | 行为 |
| --- | --- |
| Warm（快照恢复） | 从快照恢复堆状态，跳过 builtin JS 执行 |
| Cold（无快照） | 从空堆执行 builtin JS，构造 primordial 对象 |

`WJSM_STARTUP_SNAPSHOT=0` / `false` / `off` 禁用快照，强制走 cold bootstrap。用于调试和变更 bootstrap 逻辑时验证一致性。

## 快照与 GC flavor

快照保存的是对象堆字节和句柄偏移，不绑定具体 GC 算法。三种回收器（mark-sweep、G1、zgc）共用同一个 ManagedHeap 和 HandleTableV2，快照内容与 collector 选择正交。

GC 算法在 runtime 初始化后不可切换。选择优先级：

1. `--gc` CLI 选项
2. `WJSM_TEST_GC` 环境变量
3. `WJSM_GC` 环境变量
4. 默认值：`zgc`

## 嵌入工件

开发构建当前不生成嵌入工件。native cache 按需在运行时生成。`wjsm-host-native` 的 `include_bytes!` 路径用于嵌入预构建的工件（如 builtin IR 段缓存）。

快照格式版本 `SNAPSHOT_FORMAT_VERSION` 和 ABI hash 是两层校验：版本号防止 wire 格式不匹配，ABI hash 防止语义偏移。

## 深入了解

- [编译缓存](compilation-cache.md)
- [GC 选择、配置与不变量](../gc/configuration-and-invariants.md)
- [修改快照与嵌入工件](../development/changing-snapshots.md)
- [启动快照与嵌入工件](../../user/configuration/startup-snapshot.md)
