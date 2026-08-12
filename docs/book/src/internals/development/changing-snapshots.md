# 修改快照与嵌入工件

这一章说明修改 startup snapshot 格式或嵌入工件时需要改动哪些地方。

## 快照格式

startup snapshot 是 bootstrap 后的堆状态序列化。它保存：

- 对象堆字节内容；
- 句柄偏移；
- runtime 字符串表；
- native callable 表。

`SNAPSHOT_FORMAT_VERSION` 任何 wire 改动必须递增。快照 ABI hash 由 `support_abi_union_hash` + `builtin_js_bundle_hash` + `compatibility_fingerprint` 组成。

## 改动步骤

1. **格式定义**：在快照格式定义处修改序列化/反序列化逻辑。
2. **版本递增**：`SNAPSHOT_FORMAT_VERSION` 递增。
3. **ABI hash 更新**：如果快照内容影响 native cache key，更新 hash 计算。
4. **自校验**：`ManagedHeapV2ArtifactAbi` 生成时自校验，确保格式合法。
5. **cold bootstrap**：确保从空堆执行 builtin JS 能重建快照。
6. **warm restore**：确保快照恢复路径与 cold bootstrap 产出一致。
7. **测试**：`startup_snapshot.rs` 和 `embedded_startup_snapshot.rs` 测试通过。

## 禁用快照

`WJSM_STARTUP_SNAPSHOT=0` / `false` / `off` 禁用快照，每次都走 cold bootstrap。用于调试和变更 bootstrap 逻辑时验证一致性。

## 嵌入工件

当前开发构建不生成嵌入工件。native cache 按需在运行时生成。`wjsm-host-native` 的 `include_bytes!` 路径用于嵌入预构建的工件（如 builtin IR 段缓存），修改时需要同步更新 build 流程。

## 深入了解

- [构建工件索引](../reference/artifact-index.md)
- [核心不变量](../reference/invariants.md)
- [启动快照与嵌入工件](../../user/configuration/startup-snapshot.md)
