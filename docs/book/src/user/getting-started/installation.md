# 安装与升级

## 从源码构建

wjsm 没有预编译二进制分发，从源码构建是唯一安装方式。

```bash
git clone https://github.com/juzi5201314/wjsm.git
cd wjsm
cargo build --release
```

构建产物在 `target/release/wjsm`（Windows 上是 `target/release/wjsm.exe`）。

构建前请确认满足 [系统要求](requirements.md)：Rust 1.85+（2024 edition），x86_64 Linux 或 x86_64 Windows。

## 放到 PATH

把二进制复制到 `PATH` 中的目录：

```bash
# Linux
cp target/release/wjsm ~/.local/bin/

# 或 /usr/local/bin（需要 sudo）
sudo cp target/release/wjsm /usr/local/bin/
```

Windows 用户可复制到已加入 `PATH` 的目录，或手动将 `target\release` 加入 `PATH`。

验证：

```bash
wjsm version --extended
```

输出包含版本号和 Rust edition。如果 `wjsm` 命令不可用，回退到 `cargo run -- ...` 或直接调用 `target/release/wjsm ...`。

## 升级

```bash
cd wjsm
git pull
cargo build --release
```

重新构建后用 `cp` 覆盖 `PATH` 中的旧二进制即可。`target/release/wjsm` 总是最新的，不需要额外清理。

若曾设置 `WJSM_CACHE_DIR`，native cache 的 key 含 Cranelift 版本和 native ABI；升级后旧条目会自动 miss，不需要手动清理。未设置该变量时本来就没有磁盘缓存。

## 深入了解

- [运行第一个程序](first-program.md)
- [`version` 命令](../cli/version.md)
