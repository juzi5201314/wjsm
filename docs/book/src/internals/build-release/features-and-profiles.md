# Cargo Feature 与 Profile

这一章说明 workspace 的 feature 开关和编译 profile 配置。

## Features

`wjsm-host-wasm` 的 features：

```toml
[features]
default = ["embedded"]
embedded = []
```

`embedded` 是默认开启的 feature，控制是否在构建期生成嵌入工件（support cwasm、startup snapshot、artifact ABI）。`build.rs` 在 `CARGO_FEATURE_EMBEDDED` 设置时执行工件生成。

## Profile 配置

`Cargo.toml` 的 `[profile.dev]` 对关键依赖强制 `opt-level = 3`：

- `cranelift-codegen` / `cranelift-frontend` / `cranelift-codegen-meta` / `cranelift-assembler-x64`
- `wasmtime` / `wasmtime-internal-cranelift` / `wasmtime-internal-winch` / `wasmtime-internal-core` / `wasmtime-environ`
- `winch-codegen`
- `wjsm-runtime`
- `memchr`

原因：这些依赖在 debug 模式下未优化时带大量 UB 检查和未内联，严重拖慢执行。`memchr` 的 SIMD 子串搜索未优化时占 bootstrap ~35% 指令，opt 后大幅收缩。

`debug = "line-tables-only"` 保留行号信息用于 backtrace，但不保留完整调试信息，减小二进制体积。

## dev profile 的 Cranelift

dev profile 默认使用 Cranelift，但 Wasmtime 43 的 continuation 启动代码用 `naked_asm!(sym ...)`，rustup 发布的 Cranelift preview 尚不支持该操作数。因此 `wasmtime` 本体仍走 LLVM，其他依赖走 Cranelift。

## 深入了解

- [Cargo Workspace 与依赖图](workspace-and-dependencies.md)
- [`build.rs` 工件流水线](build-script.md)
- [Engine 配置与池化](../startup/engine-pool.md)
