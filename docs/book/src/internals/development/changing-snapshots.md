# 修改快照与嵌入工件

这一章说明修改启动种子格式或嵌入工件时需要改动哪些地方。

## 快照格式

`wjsm-host-native/build.rs` 写出 `startup_snapshot.bin`，再由 `include_bytes!` 嵌入。`NativeRuntime::new_*` **始终** `restore_startup_snapshot`。没有关闭开关。

现行种子包含：对象头大小的对象区、handle `0`、空 shape table、仅 `EvalIndirect` 的 host state。`SNAPSHOT_FORMAT_VERSION` 任何 wire 改动必须递增。解码时 `SnapshotExpectations` 还校验：

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

1. **格式定义**：在 `wjsm-snapshot-format` 修改编解码。
2. **版本递增**：`SNAPSHOT_FORMAT_VERSION` 递增。
3. **期望哈希**：bootstrap / codegen / semantic ABI / native ABI 任一变化都会让旧种子拒收，需要重建嵌入工件。
4. **构建期生成**：更新 `wjsm-host-native/build.rs` 的种子内容。
5. **强制 restore**：确认 `NativeRuntime::new_*` 在失配时失败，而不是跳过。
6. **测试**：快照编解码与 runtime 启动测试通过。

## 嵌入工件

开发构建把种子编进 `wjsm-host-native`。native image 磁盘缓存只在设置了 `WJSM_CACHE_DIR` 时按需生成，不替代启动种子。

## 深入了解

- [构建工件索引](../reference/artifact-index.md)
- [核心不变量](../reference/invariants.md)
- [启动快照与嵌入工件](../../user/configuration/startup-snapshot.md)
- [ADR 0003](../../../../adr/0003-startup-snapshot-boundary.md)
