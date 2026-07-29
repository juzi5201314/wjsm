# Engine 配置与池化

这一章说明 wasmtime Engine 的配置 owner 约束和进程内 Engine 池。

## 唯一 owner 约束

`crates/wjsm-host-wasm/src/engine_config.rs` 是 workspace 里唯一构造和 mutation wasmtime `Config` 的地方。所有 profile 固定开启 `threads` / `shared-memory` / `memory64` / `multi-memory` / `bulk-memory`，并保持 backtrace / address-map 不变量。

`Config` 构造不允许出现在其他 crate。这是 ADR 0011 的边界约束：wasmtime 依赖只存在于 host-wasm。

## 两种 profile

| Profile | 用途 | 配置 |
| --- | --- | --- |
| `Artifact` | support cwasm 预编译、可行性测试 | 固定 Cranelift + epoch interruption |
| `Runtime` | 运行用户代码 | 可变 compiler / opt / epoch / memory / debug |

`guest_debug` 与 Winch 不兼容，`EngineConfig::runtime` 在 `guest_debug = true` 时强制 Cranelift。这就是 `--inspect` 强制 Cranelift 的原因。

## 编译器选择

`resolve_compiler` 的优先级：

1. `RuntimeOptions.compiler`（显式参数）
2. `WJSM_COMPILER` 环境变量（`winch` / `Winch` / `WINCH`）
3. 默认 Cranelift

## 优化等级

`WJSM_OPT_LEVEL` 控制 Cranelift 优化等级：

| 值 | OptLevel |
| --- | --- |
| `none` | None |
| `speed_and_size` | SpeedAndSize |
| 未设置 / 其他 | Default |

## Engine 池

`runtime_engine_pool.rs` 维护进程内 Engine 池。`EngineConfigKey` 决定 wasmtime Config 的全部可区分维度（compiler、opt_level、epoch、memory_reservation、guest_debug）。相同 key 复用同一个 `Engine` + 一个 lazy epoch ticker；Store / Linker / RuntimeState 每次新建。

Epoch ticker 仅在有活跃 `vm` timeout（armed > 0）时每 1ms `increment_epoch()`，避免空闲时的无谓 tick。

## WASMTIME_VERSION

`WASMTIME_VERSION = "43.0.2"` 是 engine owner 绑定的精确版本，用于 benchmark evidence 和编译缓存 key。

## 深入了解

- [启动路径概览](startup-path.md)
- [编译缓存](compilation-cache.md)
- [跨 crate 所有权与依赖边界](../foundations/ownership-and-dependencies.md)
