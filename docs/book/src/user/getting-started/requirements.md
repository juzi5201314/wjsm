# 系统要求

## Rust 工具链

构建 wjsm 需要 Rust stable，2024 edition（Rust 1.85+）。这是 crate 声明 `edition = "2024"` 的硬性要求；旧版 cargo 会直接拒绝编译。

```bash
rustc --version   # 需要 >= 1.85.0
```

不需要 nightly。没有 feature gate 依赖 nightly channel。

## 平台

| 平台 | 状态 |
| --- | --- |
| x86_64 Linux | 支持 |
| x86_64 Windows | 支持 |
| 其他架构 / OS | 不支持 |

native compiler 在初始化时 fail-closed：遇到不支持的宿主直接报错退出，不切换到其他后端，也不做降级解释执行。这意味着在 aarch64 Linux 或 macOS 上 `cargo build` 能成功，但 `wjsm run` 会在 native codegen 阶段失败。

## 可选组件

| 组件 | 用途 | 何时需要 |
| --- | --- | --- |
| Test262 子模块 | ECMAScript 合规测试套件 | 运行 `cargo test` 中的 Test262 集成时 |

Test262 子模块不随默认构建拉取。需要时手动初始化：

```bash
git submodule update --init --recursive test262
```

不影响 `cargo build` 和日常使用。

## 构建

```bash
cargo build          # debug 构建，产物在 target/debug/wjsm
cargo build --release  # release 构建，产物在 target/release/wjsm
```

debug 构建可用于快速验证；日常运行和性能测试用 release 构建。

## 深入了解

- [安装与升级](installation.md)
