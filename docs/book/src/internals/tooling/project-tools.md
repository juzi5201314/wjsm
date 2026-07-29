# 项目初始化、包安装与 Shell 补全

这一章说明 `init`、`install` 和 `completions` 命令的内部实现。

## init

`init` 命令初始化一个新的 wjsm 项目。`cli_install.rs` 实现初始化逻辑：

1. 在当前目录创建 `wjsm.toml`（或检测已有配置文件）。
2. 创建 `src/` 目录和入口文件（如 `src/index.js`）。
3. 写入默认配置。

`init` 是幂等的——如果配置文件已存在，不会覆盖。

## install

`install` 命令安装 npm 包到项目。它调用 `wjsm-module` 的包安装能力：

1. 读取 `package.json` 的 dependencies。
2. 从 npm registry 下载包到 `node_modules/`。
3. 解析包的 `package.json`，注册到模块解析器。

安装的包在 `import` 时通过 `node_modules` 查找规则解析（见[包解析与条件导出](../modules/package-conditions.md)）。

## completions

`completions` 命令生成 shell 补全脚本。`clap` 的 `clap_complete` 功能生成 bash、zsh、fish、powershell 的补全脚本：

```bash
wjsm completions bash > /etc/bash_completion.d/wjsm
wjsm completions zsh > ~/.zsh/completions/_wjsm
```

补全脚本包含所有 subcommand 和选项，安装后 shell 能自动补全 `wjsm` 命令。

## 深入了解

- [CLI 参数模型与配置合并](cli-and-config.md)
- [包解析、条件与 browser 映射](../modules/package-conditions.md)
- [用户侧的 init](../../user/cli/init.md)
