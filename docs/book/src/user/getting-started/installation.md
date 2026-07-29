# 安装与升级

wjsm 目前只通过源码构建分发，仓库中没有预编译二进制、安装脚本或包管理器条目。

## 从源码构建

```bash
git clone https://github.com/juzi5201314/wjsm.git
cd wjsm
cargo build --release
```

产物是单个可执行文件 `target/release/wjsm`。它自带运行时所需的全部工件（support 模块与内置模块在构建期嵌入二进制），运行时不需要额外的运行库或 sysroot。

`cargo build` 生成的 debug 版本在 `target/debug/wjsm`，功能完全相同，但编译 JavaScript 的速度明显更慢，只适合开发调试。

## 放入 PATH

二进制可以直接复制到任意位置：

```bash
install -Dm755 target/release/wjsm ~/.local/bin/wjsm
wjsm version --extended
```

`version --extended` 会打印版本号、Rust edition、`git rev-parse --short HEAD` 的结果和目标后端：

```text
wjsm 0.1.0
  Edition: 2024
  Git: 694e72d6
  Target: wasm
```

Git 行依赖当前工作目录处于一个 Git 仓库中，且 `git` 可执行。把二进制拷到仓库外运行时这一行不会出现。

## 升级

拉取新提交后重新构建即可：

```bash
git pull
cargo build --release
```

升级会改变嵌入工件的 ABI 指纹。旧版本写下的编译缓存条目在新版本下不会被误用，但也不会自动删除；如果缓存目录占用过大，用 `wjsm cache clear` 清理。

wjsm 的版本号仍是 `0.1.0`，两次提交之间不保证 Wasm 产物格式、IR dump 文本或宿主 ABI 稳定。跨版本请重新编译 `.wasm`，不要复用旧产物。

## Test262 子模块

日常构建不需要 Test262。只有运行一致性测试时才拉取：

```bash
git submodule update --init test262
```

## 深入了解

- [构建期嵌入工件如何进入二进制](../../internals/startup/embedded-artifacts.md)
- [ABI Hash 与跨版本兼容性指纹](../../internals/startup/abi-hash.md)
- [Cargo Feature 与 Profile 对构建产物的影响](../../internals/build-release/features-and-profiles.md)
