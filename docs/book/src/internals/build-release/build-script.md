# `build.rs` 工件流水线

这一章说明 `wjsm-host-wasm` 的 `build.rs` 如何生成嵌入工件。

## 流程

`crates/wjsm-host-wasm/build.rs` 在 `embedded` feature 开启时执行三步：

1. **预编译三种 support cwasm**：对 mark-sweep、g1、zgc 三个 GC flavor，调用 `wjsm_backend_wasm::emit_support_module(flavor)` 生成 WASM，用 `wasmparser::Validator` 验证，再用 `engine.precompile_module` 编译为 cwasm，写到 `OUT_DIR/wjsm_support_{flavor}.cwasm`。
2. **计算 managed-heap V2 artifact ABI**：`engine_fingerprint` + `support_abi_hash` 组合成 `ManagedHeapV2ArtifactAbi`，自校验后写到 `OUT_DIR/wjsm_managed_heap_v2_artifact_abi.bin`。
3. **写占位文件**：`OUT_DIR/embeds.rs` 是历史保留占位，内容已被 `include_bytes!` 直接覆盖。

## 为什么复用 host-wasm 源码

build.rs 通过 `#[path = "src/engine_config.rs"]` 和 `#[path = "src/runtime_support/abi.rs"]` 直接 include host-wasm 的源码：

```rust
#[allow(dead_code)]
#[path = "src/engine_config.rs"]
mod engine_config;

#[allow(dead_code)]
#[path = "src/runtime_support/abi.rs"]
mod abi;
```

这避免把它们做成 build-dependency crate。engine 配置 owner 只有一个（ADR 0011），build.rs 复用同一份源码。

## rerun-if-changed

build.rs 声明 rerun 触发：

- `build.rs`、`src/engine_config.rs`、`src/runtime_support/abi.rs`
- `wjsm-backend-wasm/src/support_module.rs`、`wjsm-backend-wasm/src`
- `wjsm-snapshot-format/src`

这些路径变化时 cargo 重新执行 build.rs，重新生成工件。

## test262 build.rs

`crates/wjsm-test262/build.rs` 也有构建逻辑，处理 test262 测试用例的生成。详见[test262 一致性测试](../testing/test262.md)。

## 深入了解

- [生成文件与缓存边界](generated-artifacts.md)
- [构建期嵌入工件](../startup/embedded-artifacts.md)
- [版本、ABI 与兼容性](versioning-and-compatibility.md)
