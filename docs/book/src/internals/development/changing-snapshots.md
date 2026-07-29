# 修改快照与嵌入工件

这一章说明修改启动快照格式或嵌入工件时需要改动哪些地方。

## 快照格式版本

`wjsm-snapshot-format/src/lib.rs` 的 `SNAPSHOT_FORMAT_VERSION` 是格式版本号。任何 wire 改动必须递增。当前是 v9（函数属性对象新增 prototype + constructor 属性）。

## 改动步骤

1. **格式定义**：`wjsm-snapshot-format/src/lib.rs` 或 `managed_heap_v2.rs` 修改 header/section 结构。
2. **版本递增**：`SNAPSHOT_FORMAT_VERSION` 递增。
3. **编码/解码**：`encode_snapshot` / `decode_snapshot` 更新。
4. **自校验**：`ManagedHeapV2ArtifactAbi` 的自校验逻辑更新（如果 ABI 变化）。
5. **捕获/恢复**：`startup_snapshot` 模块的 `capture_startup_snapshot` 和恢复逻辑更新。
6. **build.rs**：`wjsm-host-wasm/build.rs` 重新生成嵌入工件。如果 `ManagedHeapV2ArtifactAbi` 结构变化，自校验逻辑更新。
7. **ABI 哈希**：如果快照 ABI 输入变化，`combined_abi_external_input` 更新。
8. **测试**：`crates/wjsm-runtime/tests/` 的 `startup_snapshot.rs` 和 `embedded_startup_snapshot.rs` 更新。

## 失配处理

格式版本递增后，旧快照无法解码。运行时走 cold bootstrap，build.rs 重新生成嵌入快照。这是预期行为。

`WJSM_STARTUP_SNAPSHOT_DEBUG=1` 帮助调试失配——会在 stderr 打印诊断信息。

## 嵌入工件

嵌入工件（support cwasm、artifact ABI）的改动通过 `build.rs` 的 rerun-if-changed 触发重新生成。修改 `support_module.rs`、`engine_config.rs`、`runtime_support/abi.rs` 会触发 rerun。

## 深入了解

- [启动快照格式](../startup/snapshot-format.md)
- [构建期嵌入工件](../startup/embedded-artifacts.md)
- [ABI Hash 与兼容性指纹](../startup/abi-hash.md)
