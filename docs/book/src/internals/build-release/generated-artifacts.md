# 生成文件与缓存边界

这一章说明构建系统生成的文件和缓存边界。

## OUT_DIR 下的生成文件

`wjsm-host-wasm/build.rs` 在 `OUT_DIR` 下生成：

| 文件 | 内容 |
| --- | --- |
| `wjsm_support_mark_sweep.cwasm` | mark-sweep support module 预编译产物 |
| `wjsm_support_g1.cwasm` | G1 support module 预编译产物 |
| `wjsm_support_zgc.cwasm` | ZGC support module 预编译产物 |
| `wjsm_managed_heap_v2_artifact_abi.bin` | artifact ABI 锚点 |
| `embeds.rs` | 历史保留占位 |

## src/lib.rs 的 include_bytes!

`wjsm-host-wasm/src/lib.rs` 通过 `include_bytes!` 宏在编译期把 `OUT_DIR` 下的 cwasm 嵌入二进制：

```rust
// 简化示意
const ZGC_SUPPORT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wjsm_support_zgc.cwasm"));
```

这让 `embedded_support_cwasm_for(GcAlgorithmKind::Zgc)` 能返回 `&'static [u8]`，无需运行时文件 I/O。

## 缓存边界

| 缓存 | 位置 | 失效条件 |
| --- | --- | --- |
| cargo build 缓存 | `target/` | 源码变化 |
| wasmtime 编译缓存 | `$HOME/.cache/wjsm/` | WASM 字节内容变化 |
| 启动快照 | 进程内 `OnceLock` | 进程重启 |

三者独立：cargo 缓存影响编译速度，wasmtime 缓存影响启动速度，启动快照影响 bootstrap 速度。

## /tmp 的使用

AGENTS.md 要求生成产物放在 `/tmp`，不要在仓库内创建临时文件。用户侧的 `build -o /tmp/out.wasm` 是典型用法。

## 深入了解

- [`build.rs` 工件流水线](build-script.md)
- [编译缓存](../startup/compilation-cache.md)
- [构建期嵌入工件](../startup/embedded-artifacts.md)
