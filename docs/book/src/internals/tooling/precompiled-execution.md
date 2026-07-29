# 隐藏命令与预编译执行

这一章说明 CLI 的隐藏命令和 `PrecompiledEntry` 机制。

## 隐藏命令

CLI 有一些不在 `--help` 显示的隐藏命令，用于内部工作流：

| 命令 | 用途 |
| --- | --- |
| `--precompiled` | 加载预编译 WASM，跳过编译 |
| 内部 fork handoff | 同入口 fork 时子进程直接加载 raw WASM |

隐藏命令通过 `clap` 的 `hide = true` 属性实现。它们不是用户面向的 API，可能在版本间变化。

## PrecompiledEntry

`PrecompiledEntry` 是预编译 handoff 结构：

```rust
pub struct PrecompiledEntry {
    pub source: PathBuf,
    pub wasm: PathBuf,
}
```

`source` 是源码路径，`wasm` 是预编译 WASM 路径。fork 子进程时传入这个结构，子进程直接 `Module::deserialize_file` 加载 WASM，跳过 `compile_source`。

## 使用场景

预编译执行主要用于：

- **测试**：`cargo nextest` 运行大量 fixture，每个 fixture 需要编译。预编译后 fork 子进程直接加载，减少重复编译开销。
- **嵌入式**：构建时预编译 WASM，运行时直接加载。

## 缓存与预编译的区别

编译缓存（[编译缓存](../startup/compilation-cache.md)）是运行时按 WASM 字节内容缓存，命中时 `deserialize_file` 加载。预编译执行是显式提供 WASM 路径，完全跳过编译入口。

缓存是透明的——用户不需要知道缓存存在。预编译执行是显式的——用户（或测试框架）需要提供 WASM 路径。

## 深入了解

- [源码输入与编译编排](source-input.md)
- [编译缓存](../startup/compilation-cache.md)
- [用户侧的 WASM 产物与宿主要求](../../user/output/wasm-artifacts.md)
