# 嵌入工件与多 GC flavor 切换

这一章说明 startup snapshot 如何与 GC flavor 配合，以及嵌入工件的管理。

## 启动快照

startup snapshot 是构建期 bootstrap 后的堆状态。它保存 primordial 对象（`Object.prototype`、`Array.prototype` 等）和全局构造器的状态，以 `startup_snapshot.bin` 嵌入 `wjsm-host-native`。

`NativeRuntime::new` / `new_with_config` / `new_with_inspector` / `new_with_config_and_inspector` **始终**恢复这份快照。没有 `WJSM_STARTUP_SNAPSHOT` 开关，进程启动不会走「空堆再跑一遍 builtin JS」的路径。cold bootstrap 只发生在构建嵌入工件时。

解码期望见 `SnapshotExpectations`：`bootstrap_hash`、`NATIVE_CODEGEN_HASH`、`semantic_abi_hash`、`native_abi_hash`、`{ARCH}-{OS}`、endian，以及当前堆的 `object_heap_base` / capacity。

## 快照与 GC flavor

快照保存的是对象堆字节和句柄偏移，不绑定具体 GC 算法。三种回收器（mark-sweep、G1、zgc）共用同一个 ManagedHeap 和 HandleTableV2，快照内容与 collector 选择正交。恢复时按当前 `NativeRuntimeConfig.gc_algorithm` 建 `NativeGc`，再灌入堆字节。

GC 算法在 runtime 初始化后不可切换。选择优先级：

1. `--gc` CLI 选项
2. `WJSM_TEST_GC` 环境变量
3. `WJSM_GC` 环境变量
4. 默认值：`zgc`

## 嵌入工件

快照由 `wjsm-host-native` 的 `include_bytes!` 嵌入。native cache 是另一条 opt-in 路径（`WJSM_CACHE_DIR`），按需在运行时生成 `.wnat`，不替代启动快照。

快照格式版本 `SNAPSHOT_FORMAT_VERSION` 和上述 hash 是两层校验：版本号防止 wire 格式不匹配，hash 防止语义或 ABI 偏移。

## 深入了解

- [编译缓存](compilation-cache.md)
- [GC 选择、配置与不变量](../gc/configuration-and-invariants.md)
- [修改快照与嵌入工件](../development/changing-snapshots.md)
- [启动快照与嵌入工件](../../user/configuration/startup-snapshot.md)
