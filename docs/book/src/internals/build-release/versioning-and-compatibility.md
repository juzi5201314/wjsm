# 版本、ABI 与兼容性

这一章说明 wjsm 的版本管理、ABI 兼容性和指纹机制。

## 版本

- workspace 版本：`0.1.0`（`[workspace.package] version = "0.1.0"`）。
- Rust edition：`2024`。
- wasmtime：`43.0.2`（精确版本，`WASMTIME_VERSION` 常量）。
- swc_core：`62.0.0`。

## ABI 哈希

ABI 哈希判断快照和 support cwasm 与当前 engine 是否兼容。哈希由三项组成：

1. `support_abi_union_hash()`：三种 GC flavor 的 support ABI 合并。
2. `builtin_js_bundle_hash()`：builtin JS 文件内容的哈希。
3. `compatibility_fingerprint(engine)`：wasmtime engine 配置指纹（含 `WASMTIME_VERSION`）。

任一项变化，ABI 哈希变化，快照失配。详见[ABI Hash 与兼容性指纹](../startup/abi-hash.md)。

## 快照格式版本

`SNAPSHOT_FORMAT_VERSION = 9`。v9 新增函数属性对象的 `prototype` + `constructor` 属性。任何 wire 改动必须递增版本号。

## ManagedHeapV2ArtifactAbi

`ManagedHeapV2ArtifactAbi` 记录 engine fingerprint 和 support ABI hash，是 support cwasm 工件的 ABI 锚点。build.rs 生成时自校验——解码后比较 fingerprint 和 hash，确保一致。

## 兼容性规则

- wasmtime 版本变化 → 快照失配（fingerprint 含版本）。
- builtin JS 变化 → 快照失配（bundle hash 变化）。
- support module 变化 → 快照失配（union hash 变化）。
- engine 配置变化（compiler、opt level 等）→ 快照失配（fingerprint 变化）。

## 深入了解

- [ABI Hash 与兼容性指纹](../startup/abi-hash.md)
- [启动快照格式](../startup/snapshot-format.md)
- [`build.rs` 工件流水线](build-script.md)
