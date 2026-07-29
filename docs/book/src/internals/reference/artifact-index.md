# 构建工件索引

这一章汇总构建系统生成的工件。

## build.rs 生成（`embedded` feature 开启时）

| 工件 | 路径 | 内容 |
| --- | --- | --- |
| support cwasm × 3 | `OUT_DIR/wjsm_support_{flavor}.cwasm` | 三种 GC flavor 的 support module 预编译产物 |
| artifact ABI | `OUT_DIR/wjsm_managed_heap_v2_artifact_abi.bin` | engine fingerprint + support ABI hash |
| embeds.rs | `OUT_DIR/embeds.rs` | 历史保留占位 |

## 二进制内嵌

`wjsm-host-wasm/src/lib.rs` 通过 `include_bytes!` 把 `OUT_DIR` 下的 cwasm 嵌入二进制。运行时通过 `embedded_support_cwasm_for(kind)` 返回 `&'static [u8]`。

## 运行时缓存

| 缓存 | 位置 | key |
| --- | --- | --- |
| 编译缓存 | `$WJSM_CACHE_DIR` 或 `$HOME/.cache/wjsm` | `wasmtime-43` + WASM 字节 SipHash |
| 启动快照 | 进程内 `OnceLock` | ABI 哈希校验 |

## 测试生成

`crates/wjsm-test262/build.rs` 在构建期处理 test262 测试用例，生成 Rust 测试函数。

`tests/` 的 `integration.rs` 和 `unit.rs` 通过 `build.rs` 自动生成 fixture 测试函数，名字是 `happy__foo` 或 `errors__foo`。

## 用户侧产物

| 命令 | 产物 |
| --- | --- |
| `wjsm build -o /tmp/out.wasm` | WASM 模块 |
| `wjsm dump-wat` | WAT 文本 |
| `wjsm dump-ast` | SWC AST 文本 |
| `wjsm dump-ir` | 语义 IR 文本 |

## 深入了解

- [`build.rs` 工件流水线](../build-release/build-script.md)
- [生成文件与缓存边界](../build-release/generated-artifacts.md)
- [编译缓存](../startup/compilation-cache.md)
