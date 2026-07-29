# Runtime facade 与公共 API

`wjsm-runtime` 是兼容 facade，只 re-export，不含实现。这一章说明它为什么存在以及它暴露什么。

## 规模

`crates/wjsm-runtime/src/lib.rs` 只有 25 行。它依赖三个 crate：`wjsm-host`、`wjsm-host-wasm`、`wjsm-gc`，全部通过 `pub use` 转出。

## 为什么保留

ADR 0011 把运行时拆成后端无关的 trait 和后端相关的实现后，外部消费者（CLI、嵌入者、测试）需要一个统一入口。`wjsm-runtime` 承担这个角色：它把 `wjsm-host-wasm` 的公开 API、`wjsm-gc` 的 GC 类型、`wjsm-host` 的 trait 契约聚合在一个 crate 名下。

新代码不应向它添加实现。新 API 应加在 owner crate，再在这里 `pub use`。

## 公开 API 摘要

| 类别 | 来源 crate | 典型导出 |
| --- | --- | --- |
| 执行入口 | wjsm-host-wasm | `execute`, `execute_with_options`, `execute_with_writer_with_options` |
| 编译入口 | wjsm-host-wasm | `compile_source`, `compile_source_with_debug` |
| WasmBackend | wjsm-host-wasm | `WasmBackend`（实现 `JsBackend`） |
| RuntimeOptions | wjsm-host-wasm | `RuntimeOptions`, `InspectConfig`, `PrecompiledEntry` |
| GC 类型 | wjsm-gc / wjsm-host-wasm | `GcAlgorithmKind`, `GcStats`, `CycleKind` |
| 值与句柄 | wjsm-host | `Value`, `Handle` |
| 宿主契约 | wjsm-host | `ExecContext`, `HeapContext`, `JsBackend` |
| 缓存管理 | wjsm-host-wasm | `module_cache_stats`, `clear_module_cache` |
| 嵌入工件 | wjsm-host-wasm | `embedded_support_cwasm_for`, `install_embedded_support_cwasm` |
| WASM 工具 | wjsm-host-wasm | `validate_wasm`, `wasm_section_sizes` |

## 嵌入用法

```toml
[dependencies]
wjsm-runtime = { path = "../wjsm/crates/wjsm-runtime" }
```

```rust
let wasm = wjsm_runtime::compile_source("console.log(1)")?;
wjsm_runtime::execute_with_options(&wasm, wjsm_runtime::RuntimeOptions::default()).await?;
```

用户侧的完整嵌入示例见[作为 Rust 库嵌入](../../user/workflows/embedding.md)。

## 深入了解

- [Workspace crate 地图](../foundations/crate-map.md)
- [跨 crate 所有权与依赖边界](../foundations/ownership-and-dependencies.md)
- [Engine 配置](engine-configuration.md)
