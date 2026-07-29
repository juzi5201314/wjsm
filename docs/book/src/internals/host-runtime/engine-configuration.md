# Engine 配置

这一章说明 Wasmtime Engine 如何配置，以及 `RuntimeOptions` 的字段如何映射到 Engine 设置。

## Engine 创建

`engine_config.rs` 的 `EngineConfig` 构建 `wasmtime::Engine`，按用途分两种模式：

| 模式 | 用途 | 编译器 |
| --- | --- | --- |
| `artifact` | 预编译 support cwasm（构建期） | Cranelift |
| `runtime` | 运行时实例化与执行 | Cranelift 或 Winch |

## 编译器选择

`resolve_compiler(explicit)` 的优先级：

1. 显式 `RuntimeOptions.compiler`（来自 `WJSM_COMPILER` 或程序设置）
2. `WJSM_COMPILER` 环境变量（`winch` 大小写不敏感 → Winch，其他 → Cranelift）
3. 默认 Cranelift

启用 inspector 时强制 Cranelift，`WJSM_COMPILER` 设置被忽略——Winch 不支持 guest_debug 需要的调试信息。

## 优化等级

`WJSM_OPT_LEVEL` 控制 Cranelift 优化：

| 值 | 等级 |
| --- | --- |
| 未设置 / 其他 | 默认（speed） |
| `none` | 无优化 |
| `speed_and_size` | 速度与体积 |

`OptLevelKey::from_env` 把字符串转成可哈希的 enum，用作 Engine 池的键。

## Engine 池

`runtime_engine_pool.rs` 维护 Engine 实例池，键是 `(compiler, opt_level, debug_codegen)` 三元组。同一配置的 Engine 只创建一次，后续复用。这避免重复编译 Wasmtime Engine 的开销。

`WJSM_OPT_LEVEL` 和 `WJSM_COMPILER` 通过 `OptLevelKey::from_env` / `RuntimeCompiler::from_env` 解析，与 Engine 池的键直接关联。

## Wasm features

Engine 启用的 Wasm features 由 `wasm-encoder` 产出的模块需求决定：`memory64`、`threads`、`bulk-memory`、`multi-memory`。这些在 `Cargo.toml` 的 `wasmtime` features 里显式启用。

## 深入了解

- [内存预留与 Engine 的关系](../../user/configuration/memory.md)
- [Inspector 强制 Cranelift 的原因](../../user/configuration/inspector.md)
- [Engine 池的缓存键设计](../build-release/generated-artifacts.md)
