# 构建工件索引

这一章汇总构建系统生成的工件。

## 构建期生成

开发构建当前不生成嵌入工件；native cache 只在设置了 `WJSM_CACHE_DIR` 时按需落盘。

## 运行时缓存

| 工件 | 位置 | 键 |
| --- | --- | --- |
| Native image cache | `$WJSM_CACHE_DIR/*.wnat`（未设置则关闭） | artifact digest + native ABI + codegen source hash + target + Cranelift + settings |
| Builtin IR 段缓存 | `$WJSM_CACHE_DIR/builtin_ir/`（未设置则不落盘） | sha256(version ‖ debug ‖ builtin source hashes) |
| Portable artifact | `.wjsm` 文件 | verified semantic IR |

## 测试生成

`crates/wjsm-test262/build.rs` 在构建期处理 test262 测试用例，生成 Rust 测试函数。

`tests/` 的 `integration.rs` 和 `unit.rs` 通过 `build.rs` 自动生成 fixture 测试函数，名字是 `happy__foo` 或 `errors__foo`。

## 用户侧产物

| 命令 | 产物 |
| --- | --- |
| `wjsm build -o /tmp/out.wjsm` | portable .wjsm |
| `wjsm dump-clif` | Cranelift IR 文本 |
| `wjsm dump-ast` | SWC AST 文本 |
| `wjsm dump-ir` | 语义 IR 文本 |

## 深入了解

- [`build.rs` 工件流水线](../build-release/build-script.md)
- [生成文件与缓存边界](../build-release/generated-artifacts.md)
- [编译缓存](../startup/compilation-cache.md)
