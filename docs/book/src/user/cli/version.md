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
- `Git` 由运行时调用 `git rev-parse --short HEAD` 得到，取的是**当前工作目录**所在仓库的 HEAD，不是编译时固化的提交。在非 Git 目录或没有 `git` 命令时这一行会缺失。
- `Target` 恒为 `wasm`，即唯一可用的执行后端。

> <details><summary>`Git` 那行不出现，是不是 bug？</summary>
>
> 不是。这个设计有几个 trade-off：
>
> - **好处**：可以用同一份二进制在公司不同仓库、不同时间复用，不需要为每个仓库重新构建。
> - **代价**：你看到的 Git 提交是「运行时所在目录的 HEAD」，不是「构建时固化的版本」——后者在跨工作目录时会失真。
>
> 当你把 `wjsm` 拷到 `/usr/local/bin` 这种非 Git 位置时，运行时找不到 `.git` 目录，`Git` 行自然消失。
>
> 需要确认「这个二进制是用哪份代码构建的」？查 GitHub release 的 commit hash（release 页面会写），或者自己 `cargo build` 时留意 `cargo build` 的输出。
>
> </details>

顶层还有 Clap 内置的 `-V` / `--version`，只打印 `wjsm 0.1.0`，不接受 `--extended`。

## 深入了解

- [版本、ABI 与兼容性边界](../../internals/build-release/versioning-and-compatibility.md)
