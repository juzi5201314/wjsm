# 安装与启动问题

## 构建失败

`cargo build` 需要支持 Rust 2024 Edition 的稳定版工具链。工具链过旧时报错会指向 `edition2024`，用 `rustup update stable` 解决。

首次构建会编译 Wasmtime、Cranelift 与 SWC，耗时较长且内存占用高。内存不足时降低并行度：

```bash
cargo build -j 2
```

## 找不到 wjsm 命令

仓库不提供预编译包，`cargo build` 的产物在 `target/debug/wjsm` 或 `target/release/wjsm`。直接使用完整路径，或把该目录加入 `PATH`。

## 输入文件读不到

路径错误的报错形式是：

```text
Error: Failed to read '/tmp/nope-xyz.js': No such file or directory (os error 2)
```

`run` 在文件不存在时还会检查 `package.json` 的 `scripts`，若存在同名脚本则改为执行该脚本。因此拼错文件名有时表现为「执行了别的东西」，而不是报错。

## 命令行参数被拒绝

参数错误由 Clap 报出并以退出码 `3` 结束。两个常见原因：

- `--inspect` / `--inspect-brk` 没有用 `=` 传值。写 `--inspect=9229`。
- `--max-heap-size` 等尺寸参数带了不支持的后缀。只支持 `K`、`M`、`G`（含 `KB`/`KiB` 等写法）。

## 深入了解

- [CLI 参数模型与配置合并](../../internals/tooling/cli-and-config.md)
