# 构建期嵌入工件

这一章说明 `embedded` feature 开启时 `build.rs` 生成的三个工件。

## build.rs 流程

`crates/wjsm-host-wasm/build.rs` 在 `CARGO_FEATURE_EMBEDDED` 设置时执行三步：

1. 对每个 GC flavor（mark-sweep、g1、zgc）调用 `wjsm_backend_wasm::emit_support_module(flavor)` 生成 WASM，用 wasmparser 验证，再 `engine.precompile_module` 编译为 cwasm，写到 `OUT_DIR/wjsm_support_{flavor}.cwasm`。
2. 计算 `ManagedHeapV2ArtifactAbi`（engine fingerprint + support ABI hash），自校验后写 `OUT_DIR/wjsm_managed_heap_v2_artifact_abi.bin`。
3. 写 `OUT_DIR/embeds.rs` 占位文件（内容已被 `include_bytes!` 直接覆盖）。

## 三个工件

| 工件 | 路径 | 作用 |
| --- | --- | --- |
| support cwasm × 3 | `wjsm_support_{flavor}.cwasm` | 三种 GC flavor 的 support module 预编译产物 |
| artifact ABI | `wjsm_managed_heap_v2_artifact_abi.bin` | engine fingerprint + support ABI hash 的二进制锚点 |
| embeds.rs | `embeds.rs` | 历史保留占位，`src/lib.rs` 直接 `include_bytes!` 覆盖 |

## build.rs 为什么复用 host-wasm 源码

build.rs 通过 `#[path = "src/engine_config.rs"]` 和 `#[path = "src/runtime_support/abi.rs"]` 直接 include host-wasm 的源码，避免把它们做成 build-dependency crate。这是 ADR 0011 的边界约束——engine 配置 owner 只有一个。

## rerun-if-changed

build.rs 声明了 rerun 触发：`build.rs`、`engine_config.rs`、`runtime_support/abi.rs`、`wjsm-backend-wasm/src/support_module.rs`、`wjsm-backend-wasm/src`、`wjsm-snapshot-format/src`。这些路径变化时 cargo 重新执行 build.rs，重新生成嵌入工件。

## 深入了解

- [预编译 Support cwasm](support-cwasm.md)
- [ABI Hash 与兼容性指纹](abi-hash.md)
- [启动快照边界](startup-snapshot.md)
