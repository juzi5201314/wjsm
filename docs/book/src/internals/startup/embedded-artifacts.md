# 嵌入工件与启动快照

这一章说明启动种子如何与 runtime 初始化配合。

## 启动快照

`startup_snapshot.bin` 由 `wjsm-host-native/build.rs` 生成并 `include_bytes!` 嵌入。现行载荷是种子，不是 builtin JS 录制：

- handle `0` 作为 global object，对象区只有对象头；
- 空 shape table；
- host state：0 个字符串，1 个 `EvalIndirect`。

`NativeRuntime::new_*` **始终** `restore_startup_snapshot`。`WJSM_STARTUP_SNAPSHOT` 已废止。指纹或格式失配时启动失败，没有运行时 cold bootstrap。原型对象在 restore 之后由 `ensure_intrinsic_prototypes` 分配。

解码期望见 `SnapshotExpectations`：`bootstrap_hash`、`NATIVE_CODEGEN_HASH`、`semantic_abi_hash`、`native_abi_hash`、`{ARCH}-{OS}`、endian，以及当前堆的 `object_heap_base` / capacity。

## 快照与 GC

种子不绑定 collector 实现细节。恢复时始终建 `NativeGc`（`GenerationalZgc`），再灌入对象区。collector 在 runtime 初始化后不可切换。

## 与磁盘缓存

native cache 是另一条按需生成 `.wnat` 的路径（目录经 `resolve_cache_dir()` 解析，默认回落 XDG/HOME），不替代启动种子。`wjsm-bench --cold` 只清空该缓存。

## 深入了解

- [编译缓存](compilation-cache.md)
- [GC 选择、配置与不变量](../gc/configuration-and-invariants.md)
- [修改快照与嵌入工件](../development/changing-snapshots.md)
- [启动快照与嵌入工件](../../user/configuration/startup-snapshot.md)
