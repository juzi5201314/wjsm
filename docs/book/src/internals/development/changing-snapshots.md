# 修改快照与嵌入工件

这一章说明修改 startup snapshot 格式或嵌入工件时需要改动哪些地方。

## 快照格式

startup snapshot 是 bootstrap 后的堆状态序列化，构建期写入 `startup_snapshot.bin`，由 `wjsm-host-native` `include_bytes!` 嵌入。`NativeRuntime::new_*` **始终**调用 `restore_startup_snapshot`。没有环境变量可以跳过恢复。

它保存：

- 对象堆字节内容；
- 句柄偏移与世代；
- shape table；
- runtime 字符串表与 native callable 表（host state）。

`SNAPSHOT_FORMAT_VERSION` 任何 wire 改动必须递增。解码时 `SnapshotExpectations` 还校验：

| 字段 | 来源 |
| --- | --- |
| `bootstrap_hash` | `wjsm-host-native` 构建期 `BOOTSTRAP_HASH` |
| `lowering_hash` | `wjsm_backend_native::NATIVE_CODEGEN_HASH` |
| `semantic_abi_hash` | `wjsm_artifact_format::semantic_abi_hash()` |
| `native_abi_hash` | `wjsm_native_abi::native_abi_hash()` |
| `target` | `{ARCH}-{OS}` |
| `endian` | `SnapshotEndian::current()` |
| `object_heap_base` / `object_heap_capacity_end` | 当前 `NativeHeapMemory` 布局 |

见 `crates/wjsm-host-native/src/snapshot.rs`。

## 改动步骤

1. **格式定义**：在 `wjsm-snapshot-format` 修改序列化/反序列化逻辑。
2. **版本递增**：`SNAPSHOT_FORMAT_VERSION` 递增。
3. **期望哈希**：bootstrap / codegen / semantic ABI / native ABI 任一变化都会让旧快照拒收，需要重建嵌入工件。
4. **cold bootstrap**：构建期从空堆执行 builtin JS，写出新的 `startup_snapshot.bin`。
5. **warm restore**：确认 `NativeRuntime::new_*` 恢复后的 primordial 与 cold 产出一致。
6. **测试**：快照编解码与 runtime 启动测试通过。

## 嵌入工件

开发构建把快照编进 `wjsm-host-native`。native image 磁盘缓存是另一条路径，只在设置了 `WJSM_CACHE_DIR` 时按需生成，不替代启动快照。

## 深入了解

- [构建工件索引](../reference/artifact-index.md)
- [核心不变量](../reference/invariants.md)
- [启动快照与嵌入工件](../../user/configuration/startup-snapshot.md)
