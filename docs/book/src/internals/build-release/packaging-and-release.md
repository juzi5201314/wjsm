# 打包与发布

这一章说明 wjsm 的打包和发布流程。

## 二进制

`wjsm` 是 workspace 的根 package，`wjsm-cli` 是实际的 CLI crate。发布二进制是 `wjsm` 命令：

```bash
cargo build --release
# 产物：target/release/wjsm
```

dev profile 默认使用 Cranelift（wasmtime 本体走 LLVM），release profile 的配置见[Cargo Feature 与 Profile](features-and-profiles.md)。

## embedded feature

默认构建开启 `embedded` feature，二进制内嵌三个 support cwasm 和（如果构建时生成）startup snapshot。这让单二进制无需外部文件即可运行。

不开 `embedded` 时，build.rs 跳过工件生成，运行时需要从磁盘加载 support cwasm 或走 cold bootstrap。

## 安装方式

用户侧安装方式：

| 方式 | 说明 |
| --- | --- |
| `cargo install --path .` | 从源码安装 |
| `wjsm install` | 自更新（如果有发布渠道） |
| 预编译二进制 | 下载 release 产物 |

`cli_install.rs` 实现安装逻辑，`init` 命令初始化项目。

## 发布检查清单

发布前确认：

- [ ] `cargo nextest run --workspace` 全部通过。
- [ ] `cargo build --release` 无 warning。
- [ ] embedded snapshot ABI 自校验通过（build.rs 自校验）。
- [ ] `WASMTIME_VERSION` 与实际 wasmtime 版本一致。
- [ ] 快照格式版本（`SNAPSHOT_FORMAT_VERSION`）如需变更已递增。
- [ ] `.version` 锚点更新到发布 commit。

## 深入了解

- [Cargo Feature 与 Profile](features-and-profiles.md)
- [版本、ABI 与兼容性](versioning-and-compatibility.md)
- [用户侧的安装与升级](../../user/getting-started/installation.md)
