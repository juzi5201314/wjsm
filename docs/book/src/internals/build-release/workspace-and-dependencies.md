# Cargo Workspace 与依赖图

这一章说明 wjsm 的 workspace 组织和 crate 依赖关系。

## Workspace 成员

15 个 crate 组成 workspace：

| Crate | 职责 |
| --- | --- |
| `wjsm-parser` | SWC 解析边界 |
| `wjsm-semantic` | 语义分析与 lowering |
| `wjsm-ir` | 中间表示与常量 |
| `wjsm-backend-wasm` | WASM 代码生成 |
| `wjsm-backend-jit` | JIT 后端边界（未实现的扩展点） |
| `wjsm-runtime` | 兼容 facade |
| `wjsm-cli` | 命令行接口 |
| `wjsm-test262` | test262 集成 |
| `wjsm-module` | 模块系统与 bundler |
| `wjsm-gc-bench` | GC 基准 |
| `wjsm-snapshot-format` | 快照二进制格式 |
| `wjsm-host` | 后端无关的 host trait |
| `wjsm-host-wasm` | wasmtime 后端实现 |
| `wjsm-builtins` | JavaScript builtins 算法 |
| `wjsm-gc` | 垃圾回收器 |

## 依赖边界

ADR 0011–0013 定义了依赖边界：

- `wasmtime` 依赖只在 `wjsm-host-wasm`。
- `wjsm-builtins`、`wjsm-host`、`wjsm-gc`、`wjsm-module` 不依赖后端。
- `wjsm-runtime` 是 facade，只 re-export。

## 关键依赖

workspace 共享的关键依赖：

- `swc_core 62.0.0`：解析器。
- `wasmtime =43.0.2`：WASM 运行时（精确版本）。
- `wasm-encoder 0.246.2`：WASM 编码。
- `tokio 1`：异步运行时。
- `clap 4.6.0`：CLI 参数。
- `serde 1.0` / `serde_json 1.0`：序列化。

wasmtime 的 features 显式选择，不启用 component model、stack switching、profiling、pooling allocator、coredump、内置 cache 等不使用的功能。

## 深入了解

- [Workspace crate 地图](../foundations/crate-map.md)
- [跨 crate 所有权与依赖边界](../foundations/ownership-and-dependencies.md)
- [ADR 0011–0013 的决策记录](../reference/adr-index.md)
