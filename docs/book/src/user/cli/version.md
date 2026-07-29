# version

打印版本号，可附带构建信息。

```bash
wjsm version
wjsm version --extended
```

默认只输出一行：

```text
wjsm 0.1.0
```

`--extended` 追加三行：

```text
wjsm 0.1.0
  Edition: 2024
  Git: 694e72d6
  Target: wasm
```

- `Edition` 是构建该二进制的 Rust edition。
- `Git` 由运行时调用 `git rev-parse --short HEAD` 得到，取的是**当前工作目录**所在仓库的 HEAD，
  不是编译时固化的提交。在非 Git 目录或没有 `git` 命令时这一行会缺失。
- `Target` 恒为 `wasm`，即唯一可用的执行后端。

顶层还有 Clap 内置的 `-V` / `--version`，只打印 `wjsm 0.1.0`，不接受 `--extended`。

## 深入了解

- [版本、ABI 与兼容性边界](../../internals/build-release/versioning-and-compatibility.md)
